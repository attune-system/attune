//! One ZIP interpretation, with no EOCD fallback or filename-extra overrides.
use super::{launch, limit, paths, Limits};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

struct Entry {
    name: String,
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u32,
    size: u32,
    local: u32,
    data: u64,
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn bytes(reader: &mut impl Read, length: u16) -> Result<Vec<u8>> {
    let mut bytes = vec![0; usize::from(length)];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn extra_fields(mut extra: &[u8]) -> Result<()> {
    while !extra.is_empty() {
        ensure!(extra.len() >= 4, "truncated JAR extra field");
        let tag = u16_at(extra, 0);
        let size = usize::from(u16_at(extra, 2));
        ensure!(extra.len() >= size + 4, "truncated JAR extra field");
        ensure!(tag != 0x7075, "JAR Unicode Path extra field is forbidden");
        ensure!(tag != 0x0001, "ZIP64 JAR is unsupported");
        extra = &extra[4 + size..];
    }
    Ok(())
}

fn directory(file: &mut File, limits: Limits) -> Result<Vec<Entry>> {
    let size = file.metadata()?.len();
    let tail_size = size.min(65_557);
    file.seek(SeekFrom::End(-(tail_size as i64)))?;
    let mut tail = vec![0; tail_size as usize];
    file.read_exact(&mut tail)?;
    let mut candidates = (0..tail.len().saturating_sub(21)).filter(|&i| {
        &tail[i..i + 4] == b"PK\x05\x06"
            && i + 22 + usize::from(u16_at(&tail, i + 20)) == tail.len()
    });
    let end = candidates.next().context("missing JAR ZIP end record")?;
    ensure!(candidates.next().is_none(), "ambiguous JAR ZIP end records");
    ensure!(
        !tail[end + 22..]
            .windows(4)
            .any(|bytes| bytes == b"PK\x05\x06"),
        "EOCD marker in JAR ZIP comment is forbidden"
    );
    let end_record = &tail[end..end + 22];
    let count = u16_at(end_record, 10);
    ensure!(
        u16_at(end_record, 4) == 0 && u16_at(end_record, 6) == 0 && u16_at(end_record, 8) == count,
        "split JAR ZIP is unsupported"
    );
    let directory_size = u32_at(end_record, 12);
    let directory_offset = u32_at(end_record, 16);
    ensure!(
        count != u16::MAX && directory_size != u32::MAX && directory_offset != u32::MAX,
        "ZIP64 JAR is unsupported"
    );
    let directory_offset = u64::from(directory_offset);
    let directory_size = u64::from(directory_size);
    ensure!(
        directory_offset + directory_size == size - tail_size + end as u64,
        "invalid JAR central directory bounds"
    );
    limit("jar_entries", u64::from(count), limits.entries)?;
    limit("jar_directory_bytes", directory_size, limits.manifest_bytes)?;
    ensure!(
        directory_size >= u64::from(count) * 46,
        "JAR central directory count exceeds its bytes"
    );

    file.seek(SeekFrom::Start(directory_offset))?;
    let mut directory = (&mut *file).take(directory_size);
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for _ in 0..count {
        let mut header = [0; 46];
        directory.read_exact(&mut header)?;
        ensure!(&header[..4] == b"PK\x01\x02", "invalid JAR central entry");
        let raw_name = bytes(&mut directory, u16_at(&header, 28))?;
        extra_fields(&bytes(&mut directory, u16_at(&header, 30))?)?;
        bytes(&mut directory, u16_at(&header, 32))?;
        let name = String::from_utf8(raw_name).context("JAR filename must be UTF-8")?;
        let flags = u16_at(&header, 8);
        let method = u16_at(&header, 10);
        ensure!(
            flags & !0x080e == 0 && matches!(method, 0 | 8),
            "unsupported JAR flags or compression method"
        );
        ensure!(
            name.is_ascii() || flags & 0x0800 != 0,
            "non-ASCII JAR name requires UTF-8 flag"
        );
        paths::validate(name.strip_suffix('/').unwrap_or(&name))?;
        ensure!(names.insert(name.clone()), "duplicate JAR entry");
        let mode = (u32_at(&header, 38) >> 16) & 0o170000;
        ensure!(
            matches!(mode, 0 | 0o100000 | 0o040000),
            "JAR contains a link or special file"
        );
        ensure!(u16_at(&header, 34) == 0, "split JAR entry is unsupported");
        let entry = Entry {
            name,
            flags,
            method,
            crc: u32_at(&header, 16),
            compressed: u32_at(&header, 20),
            size: u32_at(&header, 24),
            local: u32_at(&header, 42),
            data: 0,
        };
        ensure!(
            entry.compressed != u32::MAX && entry.size != u32::MAX && entry.local != u32::MAX,
            "ZIP64 JAR entry is unsupported"
        );
        limit("jar_file_bytes", u64::from(entry.size), limits.file_bytes)?;
        total += u64::from(entry.size);
        limit("jar_extracted_bytes", total, limits.extracted_bytes)?;
        if entry.name.ends_with('/') {
            ensure!(entry.size == 0, "JAR directory contains data");
        }
        if entry.name.eq_ignore_ascii_case("META-INF/MANIFEST.MF") {
            limit(
                "jar_manifest_bytes",
                u64::from(entry.size),
                limits.manifest_bytes.min(1 << 20),
            )?;
        }
        entries.push(entry);
    }
    ensure!(
        directory.limit() == 0,
        "JAR central directory has unaccounted bytes"
    );

    // Require complete, non-overlapping local records. This also excludes an
    // appended second directory/EOCD and any prepended self-extracting content.
    entries.sort_by_key(|entry| entry.local);
    let mut covered = 0;
    for entry in &mut entries {
        ensure!(
            u64::from(entry.local) == covered && covered + 30 <= directory_offset,
            "JAR local records overlap or contain gaps"
        );
        file.seek(SeekFrom::Start(covered))?;
        let mut header = [0; 30];
        file.read_exact(&mut header)?;
        ensure!(&header[..4] == b"PK\x03\x04", "invalid JAR local header");
        ensure!(
            u16_at(&header, 6) == entry.flags && u16_at(&header, 8) == entry.method,
            "JAR local and central flags/method differ"
        );
        let name_size = u16_at(&header, 26);
        let extra_size = u16_at(&header, 28);
        entry.data = covered + 30 + u64::from(name_size) + u64::from(extra_size);
        ensure!(
            entry.data + u64::from(entry.compressed) <= directory_offset,
            "JAR local payload exceeds data region"
        );
        ensure!(
            bytes(file, name_size)? == entry.name.as_bytes(),
            "JAR raw local and central names differ"
        );
        extra_fields(&bytes(file, extra_size)?)?;
        let descriptor = entry.flags & 8 != 0;
        for (offset, expected) in [(14, entry.crc), (18, entry.compressed), (22, entry.size)] {
            let value = u32_at(&header, offset);
            ensure!(
                value == expected || (descriptor && value == 0),
                "JAR local and central sizes/CRC differ"
            );
        }
        covered = entry.data + u64::from(entry.compressed);
        if descriptor {
            ensure!(
                covered + 12 <= directory_offset,
                "truncated JAR data descriptor"
            );
            file.seek(SeekFrom::Start(covered))?;
            let mut first = [0; 4];
            file.read_exact(&mut first)?;
            let signed = &first == b"PK\x07\x08";
            let mut descriptor = [0; 12];
            if signed {
                ensure!(
                    covered + 16 <= directory_offset,
                    "truncated JAR data descriptor"
                );
                file.read_exact(&mut descriptor)?;
            } else {
                descriptor[..4].copy_from_slice(&first);
                file.read_exact(&mut descriptor[4..])?;
            }
            ensure!(
                u32_at(&descriptor, 0) == entry.crc
                    && u32_at(&descriptor, 4) == entry.compressed
                    && u32_at(&descriptor, 8) == entry.size,
                "JAR data descriptor differs from central entry"
            );
            covered += if signed { 16 } else { 12 };
        }
    }
    ensure!(
        covered == directory_offset,
        "JAR has unaccounted local data"
    );
    Ok(entries)
}

fn payload(file: &mut File, entry: &Entry, capture: bool) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(entry.data))?;
    let mut input = BufReader::new(file.take(u64::from(entry.compressed)));
    let mut captured = Vec::new();
    let mut crc = flate2::Crc::new();
    let mut actual = 0u64;
    let mut consume = |bytes: &[u8]| -> Result<()> {
        actual += bytes.len() as u64;
        ensure!(
            actual <= u64::from(entry.size),
            "JAR entry exceeds declared size"
        );
        crc.update(bytes);
        if capture {
            captured.extend_from_slice(bytes);
        }
        Ok(())
    };
    let mut buffer = [0; 64 * 1024];
    if entry.method == 0 {
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            consume(&buffer[..n])?;
        }
    } else {
        let mut decoder = flate2::Decompress::new(false);
        loop {
            let before_in = decoder.total_in();
            let before_out = decoder.total_out();
            let status = decoder.decompress(
                input.fill_buf()?,
                &mut buffer,
                flate2::FlushDecompress::None,
            )?;
            let read = (decoder.total_in() - before_in) as usize;
            let written = (decoder.total_out() - before_out) as usize;
            input.consume(read);
            consume(&buffer[..written])?;
            if status == flate2::Status::StreamEnd {
                ensure!(
                    decoder.total_in() == u64::from(entry.compressed),
                    "trailing JAR DEFLATE bytes"
                );
                break;
            }
            ensure!(read + written > 0, "truncated JAR DEFLATE stream");
        }
    }
    ensure!(
        actual == u64::from(entry.size) && crc.sum() == entry.crc,
        "JAR payload size/CRC mismatch"
    );
    Ok(captured)
}

pub(super) fn validate(path: &Path, main_required: bool, limits: Limits) -> Result<()> {
    let mut file = File::open(path)?;
    let entries = directory(&mut file, limits)?;
    let mut manifest = None;
    for entry in entries {
        let is_manifest = entry.name.eq_ignore_ascii_case("META-INF/MANIFEST.MF");
        ensure!(!is_manifest || manifest.is_none(), "duplicate JAR manifest");
        let bytes = payload(&mut file, &entry, is_manifest)?;
        if is_manifest {
            manifest = Some(String::from_utf8(bytes)?);
        }
    }
    let mut main = None;
    if let Some(text) = manifest {
        // Check physical bytes before normalizing terminators or unfolding lines.
        let mut remaining = text.as_str();
        while !remaining.is_empty() {
            let end = remaining
                .find(['\r', '\n'])
                .context("unterminated JAR manifest physical line")?;
            ensure!(end <= 72, "JAR manifest physical line exceeds 72 bytes");
            let terminator = &remaining[end..];
            remaining = terminator.strip_prefix("\r\n").unwrap_or(&terminator[1..]);
        }
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut unfolded: Vec<String> = Vec::new();
        for line in text.lines() {
            if let Some(continuation) = line.strip_prefix(' ') {
                unfolded
                    .last_mut()
                    .context("invalid JAR manifest continuation")?
                    .push_str(continuation);
            } else {
                unfolded.push(line.into());
            }
        }
        let mut main_section = true;
        let mut keys = BTreeSet::new();
        for line in unfolded {
            if line.is_empty() {
                main_section = false;
                keys.clear();
                continue;
            }
            let (key, value) = line
                .split_once(": ")
                .context("invalid JAR manifest attribute")?;
            let key = key.to_ascii_lowercase();
            ensure!(keys.insert(key.clone()), "duplicate JAR manifest attribute");
            ensure!(
                key != "class-path" || value.trim().is_empty(),
                "JAR manifest Class-Path is not supported; declare classpath artifacts explicitly"
            );
            if main_section && key == "main-class" {
                launch::java_class(value)?;
                main = Some(value.to_owned());
            }
        }
    }
    ensure!(
        !main_required || main.is_some(),
        "java_jar requires Main-Class in META-INF/MANIFEST.MF"
    );
    Ok(())
}
