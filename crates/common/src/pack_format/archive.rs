use super::*;
use anyhow::{Context, Result};
use miniz_oxide::{
    deflate::{core::CompressorOxide, stream::deflate},
    DataFormat, MZFlush, MZStatus,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, Write};

const GZIP_HEADER: [u8; 10] = [31, 139, 8, 0, 0, 0, 0, 0, 2, 255];

pub(super) fn header(path: &str, size: u64, mode: FileMode) -> Result<[u8; 512]> {
    paths::validate(path)?;
    let (prefix, name) = paths::split_ustar(path)?;
    let mut header = [0; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    for (range, value) in [
        (100..108, mode.octal()),
        (108..116, 0),
        (116..124, 0),
        (124..136, size),
        (136..148, 0),
        (329..337, 0),
        (337..345, 0),
    ] {
        let width = range.len() - 1;
        let octal = format!("{value:0width$o}");
        ensure!(octal.len() == width, "numeric field does not fit ustar");
        header[range.start..range.end - 1].copy_from_slice(octal.as_bytes());
    }
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|b| *b as u64).sum();
    header[148..154].copy_from_slice(format!("{checksum:06o}").as_bytes());
    header[154] = 0;
    Ok(header)
}

pub(super) fn write(
    root: &Path,
    manifest: &Manifest,
    output: &mut File,
    limits: Limits,
) -> Result<()> {
    let manifest_bytes = serde_jcs::to_vec(manifest)?;
    limit(
        "manifest_bytes",
        manifest_bytes.len() as u64,
        limits.manifest_bytes,
    )?;
    limit("file_bytes", manifest_bytes.len() as u64, limits.file_bytes)?;
    let extracted =
        manifest.files.iter().map(|f| f.size).sum::<u64>() + manifest_bytes.len() as u64;
    limit("extracted_bytes", extracted, limits.extracted_bytes)?;
    limit("entries", manifest.files.len() as u64 + 1, limits.entries)?;
    let mut entries: Vec<_> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.size, f.mode))
        .collect();
    entries.push((MANIFEST_PATH, manifest_bytes.len() as u64, FileMode::Data));
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut tar = tempfile::tempfile()?;
    for (path, size, mode) in entries {
        tar.write_all(&header(
            &format!("{}/{path}", manifest.pack.r#ref),
            size,
            mode,
        )?)?;
        if path == MANIFEST_PATH {
            tar.write_all(&manifest_bytes)?;
        } else {
            let copied = io::copy(&mut File::open(root.join(path))?.take(size), &mut tar)?;
            ensure!(copied == size, "snapshot file changed");
        }
        tar.write_all(&[0; 512][..((512 - size % 512) % 512) as usize])?;
    }
    tar.write_all(&[0; 1024])?;
    tar.rewind()?;
    compress(&mut tar, output, limits.compressed_bytes)
}

// Call miniz directly so Cargo feature unification cannot switch the encoder.
fn compress(input: &mut File, output: &mut File, maximum: u64) -> Result<()> {
    let mut compressor = Box::<CompressorOxide>::default();
    compressor.set_format_and_level(DataFormat::Raw, 9);
    let mut crc = flate2::Crc::new();
    let mut compressed = 10;
    limit("compressed_bytes", compressed, maximum)?;
    output.write_all(&GZIP_HEADER)?;
    let mut input_buffer = [0; 64 * 1024];
    let mut output_buffer = [0; 64 * 1024];
    loop {
        let n = input.read(&mut input_buffer)?;
        crc.update(&input_buffer[..n]);
        let flush = if n == 0 {
            MZFlush::Finish
        } else {
            MZFlush::None
        };
        let mut remaining = &input_buffer[..n];
        loop {
            let result = deflate(&mut compressor, remaining, &mut output_buffer, flush);
            let status = result
                .status
                .map_err(|e| anyhow::anyhow!("miniz encoding failed: {e:?}"))?;
            compressed += result.bytes_written as u64;
            limit("compressed_bytes", compressed, maximum)?;
            output.write_all(&output_buffer[..result.bytes_written])?;
            remaining = &remaining[result.bytes_consumed..];
            if status == MZStatus::StreamEnd {
                limit("compressed_bytes", compressed + 8, maximum)?;
                output.write_all(&crc.sum().to_le_bytes())?;
                output.write_all(&crc.amount().to_le_bytes())?;
                return Ok(());
            }
            if remaining.is_empty() && n != 0 {
                break;
            }
            ensure!(
                result.bytes_consumed + result.bytes_written > 0,
                "encoder made no progress"
            );
        }
    }
}

struct MeasuredReader {
    file: File,
    hash: Sha256,
    count: u64,
    maximum: u64,
}

impl Read for MeasuredReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = buffer
            .len()
            .min(self.maximum.saturating_sub(self.count).saturating_add(1) as usize);
        let n = self.file.read(&mut buffer[..length])?;
        self.count += n as u64;
        if self.count > self.maximum {
            return Err(io::Error::other(format!(
                "release_limit_exceeded: compressed_bytes: {} > {}",
                self.count, self.maximum
            )));
        }
        self.hash.update(&buffer[..n]);
        Ok(n)
    }
}

pub(super) fn verify(path: &Path, limits: Limits) -> Result<VerifiedRelease> {
    ensure!(
        fs::metadata(path)?.is_file(),
        "archive must be a regular file"
    );
    let file = File::open(path)?;
    ensure!(file.metadata()?.is_file(), "archive must be a regular file");
    limit(
        "compressed_bytes",
        file.metadata()?.len(),
        limits.compressed_bytes,
    )?;
    let mut input = BufReader::new(MeasuredReader {
        file,
        hash: Sha256::new(),
        count: 0,
        maximum: limits.compressed_bytes,
    });
    ensure!(
        input.fill_buf()?.starts_with(&GZIP_HEADER),
        "noncanonical gzip header"
    );
    let mut decoder = flate2::bufread::GzDecoder::new(input);
    let staging = tempfile::tempdir()?;
    let mut inventory = Vec::new();
    let mut names = paths::Paths::default();
    let mut previous = String::new();
    let mut pack_root: Option<String> = None;
    let mut manifest = None;
    let mut total = 0u64;
    let mut largest = 0;
    let mut count = 0;
    loop {
        let mut block = [0; 512];
        decoder
            .read_exact(&mut block)
            .context("truncated TAR header/end marker")?;
        if block == [0; 512] {
            decoder
                .read_exact(&mut block)
                .context("missing second TAR end block")?;
            ensure!(block == [0; 512], "invalid TAR end marker");
            let mut extra = [0; 1];
            ensure!(decoder.read(&mut extra)? == 0, "trailing TAR bytes");
            break;
        }
        count += 1;
        limit("entries", count, limits.entries)?;
        let name = text_field(&block[..100])?;
        let prefix = text_field(&block[345..500])?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        names.insert(&path)?;
        ensure!(previous < path, "archive entries are not in bytewise order");
        previous = path.clone();
        let (root, relative) = path
            .split_once('/')
            .context("archive lacks pack root directory")?;
        match &pack_root {
            Some(expected) => ensure!(expected == root, "multiple archive roots"),
            None => {
                crate::schema::RefValidator::validate_pack_ref(root)?;
                pack_root = Some(root.to_owned());
            }
        }
        let size = octal(&block[124..136])?;
        let mode = match octal(&block[100..108])? {
            0o644 => FileMode::Data,
            0o755 => FileMode::Executable,
            _ => anyhow::bail!("invalid file mode"),
        };
        ensure!(
            block == header(&path, size, mode)?,
            "noncanonical ustar header: {path}"
        );
        limit("file_bytes", size, limits.file_bytes)?;
        total = total.checked_add(size).context("extracted size overflow")?;
        limit("extracted_bytes", total, limits.extracted_bytes)?;
        largest = largest.max(size);
        if relative == MANIFEST_PATH {
            limit("manifest_bytes", size, limits.manifest_bytes)?;
            ensure!(mode == FileMode::Data, "manifest must be non-executable");
        }
        let destination = staging.path().join(relative);
        fs::create_dir_all(destination.parent().context("missing file parent")?)?;
        let mut output = File::create_new(&destination)?;
        let mut hash = Sha256::new();
        let mut remaining = size;
        let mut buffer = [0; 64 * 1024];
        while remaining > 0 {
            let n = (remaining as usize).min(buffer.len());
            decoder
                .read_exact(&mut buffer[..n])
                .context("truncated TAR file")?;
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
            remaining -= n as u64;
        }
        let padding = ((512 - size % 512) % 512) as usize;
        decoder.read_exact(&mut buffer[..padding])?;
        ensure!(
            buffer[..padding].iter().all(|b| *b == 0),
            "nonzero TAR padding"
        );
        if relative == MANIFEST_PATH {
            let bytes = fs::read(destination)?;
            let parsed: Manifest =
                serde_json::from_slice(&bytes).context("invalid release/v1 manifest")?;
            // Comparing the exact canonical representation also rejects duplicate map keys.
            ensure!(
                serde_jcs::to_vec(&parsed)? == bytes,
                "manifest is not canonical RFC 8785 JSON"
            );
            manifest = Some(parsed);
        } else {
            inventory.push(FileEntry {
                path: relative.into(),
                size,
                sha256: hex(&hash.finalize()),
                mode,
            });
        }
    }
    let mut input = decoder.into_inner();
    ensure!(
        input.fill_buf()?.is_empty(),
        "trailing gzip member or bytes"
    );
    let measured = input.into_inner();
    let manifest = manifest.context("missing release manifest")?;
    ensure!(
        pack_root.as_deref() == Some(&manifest.pack.r#ref),
        "archive root differs from manifest"
    );
    ensure!(
        inventory == manifest.files,
        "manifest inventory differs from payload sizes, digests, paths or modes"
    );
    source::validate(staging.path(), &manifest, limits)?;
    Ok(VerifiedRelease {
        manifest,
        sha256: hex(&measured.hash.finalize()),
        compressed_size: measured.count,
        extracted_size: total,
        largest_file: largest,
        entry_count: count,
    })
}

fn text_field(bytes: &[u8]) -> Result<&str> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    ensure!(
        bytes[end..].iter().all(|b| *b == 0),
        "nonzero text field padding"
    );
    Ok(std::str::from_utf8(&bytes[..end])?)
}

fn octal(bytes: &[u8]) -> Result<u64> {
    ensure!(
        bytes.last() == Some(&0)
            && bytes[..bytes.len() - 1]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b)),
        "noncanonical octal field"
    );
    Ok(u64::from_str_radix(
        std::str::from_utf8(&bytes[..bytes.len() - 1])?,
        8,
    )?)
}
