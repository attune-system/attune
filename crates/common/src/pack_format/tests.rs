use super::*;
use std::fs;
use std::io::{Read, Write};

fn put(root: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn jar(main: bool, class_path: bool) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
    zip.write_all(b"Manifest-Version: 1.0\r\n").unwrap();
    if main {
        zip.write_all(b"Main-Class: example.Main\r\n").unwrap();
    }
    if class_path {
        zip.write_all(b"Class-Path: https://example.invalid/evil.jar\r\n")
            .unwrap();
    }
    zip.write_all(b"\r\n").unwrap();
    zip.start_file("example/Main.class", options).unwrap();
    zip.write_all(b"\xca\xfe\xba\xbe\x00\x00\x00\x41").unwrap();
    zip.finish().unwrap().into_inner()
}

fn unicode_path_extra(name: &str, override_name: &str) -> Vec<u8> {
    let mut crc = flate2::Crc::new();
    crc.update(name.as_bytes());
    let mut extra = Vec::new();
    extra.extend(0x7075u16.to_le_bytes());
    extra.extend((5 + override_name.len() as u16).to_le_bytes());
    extra.push(1);
    extra.extend(crc.sum().to_le_bytes());
    extra.extend(override_name.as_bytes());
    extra
}

// Independent stored-ZIP producer for parser-differential regression inputs.
fn adversarial_jar(rename: bool, comment_size: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut directory = Vec::new();
    for (name, content, override_name) in [
        ("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\r\nMain-Class: Main\r\nClass-Path: https://example.invalid/evil.jar\r\n\r\n", "hidden.txt"),
        ("alternate.txt", "Manifest-Version: 1.0\r\nMain-Class: Main\r\n\r\n", "META-INF/MANIFEST.MF"),
    ] {
        let content = if rename { content } else { "Manifest-Version: 1.0\r\nMain-Class: Main\r\n\r\n" };
        let extra = if rename { unicode_path_extra(name, override_name) } else { vec![] };
        let offset = bytes.len() as u32;
        let mut crc = flate2::Crc::new();
        crc.update(content.as_bytes());
        let mut local = [0u8; 30];
        local[..4].copy_from_slice(b"PK\x03\x04");
        local[4..6].copy_from_slice(&20u16.to_le_bytes());
        local[14..18].copy_from_slice(&crc.sum().to_le_bytes());
        local[18..22].copy_from_slice(&(content.len() as u32).to_le_bytes());
        local[22..26].copy_from_slice(&(content.len() as u32).to_le_bytes());
        local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        local[28..30].copy_from_slice(&(extra.len() as u16).to_le_bytes());
        bytes.extend(local);
        bytes.extend(name.as_bytes());
        bytes.extend(&extra);
        bytes.extend(content.as_bytes());
        let mut central = [0u8; 46];
        central[..4].copy_from_slice(b"PK\x01\x02");
        central[4..6].copy_from_slice(&20u16.to_le_bytes());
        central[6..8].copy_from_slice(&20u16.to_le_bytes());
        central[16..28].copy_from_slice(&local[14..26]);
        central[28..32].copy_from_slice(&local[26..30]);
        central[32..34].copy_from_slice(&comment_size.to_le_bytes());
        central[42..46].copy_from_slice(&offset.to_le_bytes());
        directory.extend(central);
        directory.extend(name.as_bytes());
        directory.extend(&extra);
        directory.resize(directory.len() + comment_size as usize, b'x');
    }
    let offset = bytes.len() as u32;
    let size = directory.len() as u32;
    bytes.extend(directory);
    let mut end = [0u8; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    end[8..10].copy_from_slice(&2u16.to_le_bytes());
    end[10..12].copy_from_slice(&2u16.to_le_bytes());
    end[12..16].copy_from_slice(&size.to_le_bytes());
    end[16..20].copy_from_slice(&offset.to_le_bytes());
    bytes.extend(end);
    bytes
}

#[test]
fn jar_unicode_path_override_cannot_hide_the_jvm_manifest() {
    let bytes = adversarial_jar(true, 0);
    let (dir, options) = fixture();
    put(&options.source, "dist/app.jar", bytes);
    let error = build(&options, &dir.path().join("bad.gz"), Limits::default()).unwrap_err();
    assert!(
        error.to_string().contains("Unicode Path extra field"),
        "{error:#}"
    );
}

#[test]
fn jar_fake_eocd_cannot_bypass_the_directory_budget() {
    let mut bytes = adversarial_jar(false, 4096);
    let limits = Limits {
        manifest_bytes: 4096,
        ..Limits::default()
    };
    let check = tempfile::NamedTempFile::new().unwrap();
    fs::write(check.path(), &bytes).unwrap();
    let error = super::jar::validate(check.path(), true, limits).unwrap_err();
    assert!(error
        .to_string()
        .contains("release_limit_exceeded: jar_directory_bytes"));
    let mut fake = [0u8; 22];
    fake[..4].copy_from_slice(b"PK\x05\x06");
    fake[8..10].copy_from_slice(&2u16.to_le_bytes());
    fake[10..12].copy_from_slice(&2u16.to_le_bytes());
    fake[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    bytes.extend(fake);
    let (dir, options) = fixture();
    put(&options.source, "dist/app.jar", bytes);
    let error = build(&options, &dir.path().join("bad.gz"), limits).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("central directory count exceeds its bytes"),
        "{error:#}"
    );
}

#[test]
fn jar_comment_cannot_hide_an_alternate_directory_with_a_trailing_byte() {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let name = b"META-INF/MANIFEST.MF";
    zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
    zip.write_all(b"Manifest-Version: 1.0\r\nMain-Class: Main\r\n\r\n")
        .unwrap();
    zip.start_file("hidden.txt", options).unwrap();
    zip.write_all(b"Manifest-Version: 1.0\r\nMain-Class: Main\r\nClass-Path: https://example.invalid/evil.jar\r\n\r\n").unwrap();
    let mut bytes = zip.finish().unwrap().into_inner();
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), &bytes).unwrap();
    super::jar::validate(file.path(), true, Limits::default()).unwrap();

    let primary_end = bytes.len() - 22;
    let directory = u32::from_le_bytes(
        bytes[primary_end + 16..primary_end + 20]
            .try_into()
            .unwrap(),
    ) as usize;
    let first_size = 46
        + [28, 30, 32]
            .iter()
            .map(|offset| {
                u16::from_le_bytes(
                    bytes[directory + offset..directory + offset + 2]
                        .try_into()
                        .unwrap(),
                ) as usize
            })
            .sum::<usize>();
    let hidden = directory + first_size;
    let hidden_name_size =
        u16::from_le_bytes(bytes[hidden + 28..hidden + 30].try_into().unwrap()) as usize;
    assert_eq!(
        &bytes[hidden + 46..hidden + 46 + hidden_name_size],
        b"hidden.txt"
    );
    let mut alternate = bytes[hidden..hidden + 46].to_vec();
    alternate[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
    alternate.extend(name);
    alternate.extend_from_slice(&bytes[hidden + 46 + hidden_name_size..primary_end]);

    let mut end = [0; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    end[8..10].copy_from_slice(&1u16.to_le_bytes());
    end[10..12].copy_from_slice(&1u16.to_le_bytes());
    end[12..16].copy_from_slice(&(alternate.len() as u32).to_le_bytes());
    end[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    // The primary comment ends at EOF; the alternate EOCD ends one byte earlier.
    bytes[primary_end + 20..primary_end + 22]
        .copy_from_slice(&((alternate.len() + 23) as u16).to_le_bytes());
    bytes.extend(alternate);
    bytes.extend(end);
    bytes.push(b'X');
    fs::write(file.path(), bytes).unwrap();
    let error = super::jar::validate(file.path(), true, Limits::default()).unwrap_err();
    assert!(
        error.to_string().contains("EOCD marker in JAR ZIP comment"),
        "{error:#}"
    );
}

fn jar_with_manifest(manifest: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(
        "META-INF/MANIFEST.MF",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(manifest.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn jar_manifest_requires_a_final_physical_line_terminator() {
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        file.path(),
        jar_with_manifest("Manifest-Version: 1.0\r\nMain-Class: Main"),
    )
    .unwrap();
    let error = super::jar::validate(file.path(), true, Limits::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unterminated JAR manifest physical line"),
        "{error:#}"
    );
}

#[test]
fn jar_manifest_rejects_overlong_physical_lines_in_bytes() {
    let file = tempfile::NamedTempFile::new().unwrap();
    for value in ["x".repeat(600), "x".repeat(70), "\u{e9}".repeat(35)] {
        let manifest = format!("Manifest-Version: 1.0\r\nMain-Class: Main\r\nX: {value}\r\n\r\n");
        fs::write(file.path(), jar_with_manifest(&manifest)).unwrap();
        let error = super::jar::validate(file.path(), true, Limits::default()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("JAR manifest physical line exceeds 72 bytes"),
            "{error:#}"
        );
    }
}

#[test]
fn jar_manifest_accepts_lf_crlf_cr_and_bounded_continuations() {
    let file = tempfile::NamedTempFile::new().unwrap();
    for newline in ["\n", "\r\n", "\r"] {
        let manifest = format!("Manifest-Version: 1.0{newline}Main-Class: Ma{newline} in{newline}X: {}{newline} {}{newline}{newline}", "x".repeat(69), "x".repeat(71));
        fs::write(file.path(), jar_with_manifest(&manifest)).unwrap();
        super::jar::validate(file.path(), true, Limits::default()).unwrap();
    }
}

#[test]
fn jar_raw_local_names_and_extra_fields_are_checked_too() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut bytes = adversarial_jar(false, 0);
    bytes[30] = b'X';
    fs::write(file.path(), bytes).unwrap();
    let error = super::jar::validate(file.path(), true, Limits::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("raw local and central names differ"),
        "{error:#}"
    );

    let mut bytes = adversarial_jar(true, 0);
    let offsets: Vec<_> = bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, w)| (w == b"PK\x01\x02").then_some(i))
        .collect();
    for offset in offsets {
        let name_size =
            u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
        let extra = offset + 46 + name_size;
        bytes[extra..extra + 2].copy_from_slice(&0xffffu16.to_le_bytes());
    }
    fs::write(file.path(), bytes).unwrap();
    let error = super::jar::validate(file.path(), true, Limits::default()).unwrap_err();
    assert!(
        error.to_string().contains("Unicode Path extra field"),
        "{error:#}"
    );
}

#[test]
fn archive_verifier_rejects_unicode_renaming_with_correct_payload_digests() {
    use sha2::{Digest, Sha256};
    let (dir, options) = fixture();
    let snapshot = tempfile::tempdir().unwrap();
    let mut manifest = source::collect(&options, snapshot.path(), Limits::default()).unwrap();
    let bytes = adversarial_jar(true, 0);
    let path = ".attune/artifacts/app/portable";
    put(snapshot.path(), path, &bytes);
    let entry = manifest.files.iter_mut().find(|f| f.path == path).unwrap();
    entry.size = bytes.len() as u64;
    entry.sha256 = hex(&Sha256::digest(&bytes));
    let archive = dir.path().join("bad.gz");
    archive::write(
        snapshot.path(),
        &manifest,
        &mut fs::File::create(&archive).unwrap(),
        Limits::default(),
    )
    .unwrap();
    let error = verify(&archive, Limits::default()).unwrap_err();
    assert!(
        error.to_string().contains("Unicode Path extra field"),
        "{error:#}"
    );
}

#[test]
fn jar_deflate_streams_are_checked_for_size_crc_and_complete_records() {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
    zip.write_all(b"Manifest-Version: 1.0\r\nMain-Class: Main\r\n\r\n")
        .unwrap();
    zip.start_file("Main.class", options).unwrap();
    zip.write_all(&vec![b'x'; 3 * 64 * 1024 + 17]).unwrap();
    zip.set_comment("ordinary ZIP comment");
    let valid = zip.finish().unwrap().into_inner();
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), &valid).unwrap();
    super::jar::validate(file.path(), true, Limits::default()).unwrap();
    let data = zip::ZipArchive::new(std::io::Cursor::new(&valid))
        .unwrap()
        .by_index(1)
        .unwrap()
        .data_start() as usize;
    let mut corrupt = valid.clone();
    corrupt[data] ^= 0xff;
    fs::write(file.path(), corrupt).unwrap();
    assert!(super::jar::validate(file.path(), true, Limits::default()).is_err());

    let end = valid.windows(4).position(|w| w == b"PK\x05\x06").unwrap();
    let directory = u32::from_le_bytes(valid[end + 16..end + 20].try_into().unwrap()) as usize;
    for mutation in 0..3 {
        let mut corrupt = valid.clone();
        match mutation {
            0 => {
                corrupt[end + 8..end + 10].copy_from_slice(&1u16.to_le_bytes());
                corrupt[end + 10..end + 12].copy_from_slice(&1u16.to_le_bytes());
            }
            1 => corrupt[30] = b'X',
            2 => {
                corrupt[directory + 16] ^= 1;
                corrupt[14] ^= 1;
            }
            _ => unreachable!(),
        }
        fs::write(file.path(), corrupt).unwrap();
        assert!(
            super::jar::validate(file.path(), true, Limits::default()).is_err(),
            "mutation {mutation}"
        );
    }
}

fn fixture() -> (tempfile::TempDir, BuildOptions) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    put(
        &root,
        "pack.yaml",
        "ref: demo\nversion: 1.2.3\nlabel: Demo\nconf_schema:\n  message:\n    type: string\n",
    );
    put(&root, ".gitignore", "dist/\n");
    put(&root, "actions/native.yaml", "ref: demo.native\nrunner_type: native\nlaunch:\n  type: native\n  artifact: tool\nparam_schema:\n  message:\n    type: string\n");
    put(&root, "actions/jar.yaml", "ref: demo.jar\nrunner_type: java\nlaunch:\n  type: java_jar\n  artifact: app\n  jvm_args: ['-Xmx64m']\n");
    put(&root, "actions/classes.yaml", "ref: demo.classes\nrunner_type: java\nlaunch:\n  type: java_class\n  main_class: example.Main\n  classpath:\n    - path: classes\n    - artifact: app\n");
    put(
        &root,
        "classes/example/Main.class",
        b"\xca\xfe\xba\xbe\x00\x00\x00\x41",
    );
    put(&root, "helper", b"#!/bin/sh\nexit 0\n");
    put(&root, "README.md", b"payload\r\nbytes stay unchanged\r\n");
    put(
        &root,
        "cafe\u{301}.txt",
        b"decomposed source filename, unchanged payload\r\n",
    );
    // Opaque native payloads are never executed or rewritten by the builder.
    put(
        &root,
        "dist/tool-amd64",
        b"\x7fELF\x02\x01\x01\x00amd64-fixture\n",
    );
    put(
        &root,
        "dist/tool-arm64",
        b"\x7fELF\x02\x01\x01\x00arm64-fixture\n",
    );
    put(&root, "dist/app.jar", jar(true, false));
    let options = BuildOptions {
        source: root.clone(),
        executables: vec!["helper".into()],
        artifacts: vec![
            format!(
                "tool[linux/arm64/static]={}/dist/tool-arm64",
                root.display()
            )
            .parse()
            .unwrap(),
            format!("app={}/dist/app.jar", root.display())
                .parse()
                .unwrap(),
            format!(
                "tool[linux/amd64/static]={}/dist/tool-amd64",
                root.display()
            )
            .parse()
            .unwrap(),
        ],
    };
    (dir, options)
}

#[test]
fn pack_format_golden() {
    let (dir, mut options) = fixture();
    let a = dir.path().join("a.tar.gz");
    let result = build(&options, &a, Limits::default()).unwrap();
    assert_eq!(
        (
            result.sha256.as_str(),
            result.compressed_size,
            result.extracted_size
        ),
        (
            "46c80ccd915d412f051175384522d50f9bbc489bbea31ce7fe1a5dc0a4986761",
            1865,
            3188
        )
    );
    let manifest = result.manifest();
    assert!(manifest.files.iter().any(|f| f.path == "caf\u{e9}.txt"));
    assert!(!manifest
        .files
        .iter()
        .any(|f| f.path.starts_with("dist/") || f.path == MANIFEST_PATH));
    assert_eq!(
        manifest
            .files
            .iter()
            .find(|f| f.path == "helper")
            .unwrap()
            .mode,
        FileMode::Executable
    );
    assert_eq!(
        manifest
            .files
            .iter()
            .find(|f| f.path == "README.md")
            .unwrap()
            .mode,
        FileMode::Data
    );
    assert_eq!(
        manifest.artifacts[0].component_refs(),
        ["demo.classes", "demo.jar"]
    );
    let originals: Vec<_> = walkdir::WalkDir::new(&options.source)
        .into_iter()
        .map(Result::unwrap)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path().strip_prefix(&options.source).unwrap().to_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    let new_source = dir.path().join("reordered");
    for (path, bytes) in originals.iter().rev() {
        put(&new_source, path.to_str().unwrap(), bytes);
        let file = fs::File::options()
            .write(true)
            .open(new_source.join(path))
            .unwrap();
        file.set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(123456)),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(new_source.join(path), fs::Permissions::from_mode(0o777)).unwrap();
        }
    }
    for input in &mut options.artifacts {
        let source = match input {
            ArtifactInput::Native { source, .. } | ArtifactInput::Jar { source, .. } => source,
        };
        *source = new_source.join(source.strip_prefix(&options.source).unwrap());
    }
    options.source = new_source;
    options.artifacts.reverse();
    let b = dir.path().join("b.tar.gz");
    let second = build(&options, &b, Limits::default()).unwrap();
    assert_eq!(result.sha256, second.sha256);
    assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap());
}

#[test]
fn every_format_limit_is_enforced_at_boundary() {
    let (dir, options) = fixture();
    let archive = dir.path().join("release.tar.gz");
    let result = build(&options, &archive, Limits::default()).unwrap();
    let manifest_size = serde_jcs::to_vec(result.manifest()).unwrap().len() as u64;
    let exact = Limits {
        compressed_bytes: result.compressed_size,
        extracted_bytes: result.extracted_size,
        file_bytes: result.largest_file,
        entries: result.entry_count,
        manifest_bytes: manifest_size,
        artifacts: 2,
        variants_per_artifact: 2,
    };
    verify(&archive, exact).unwrap();
    for (name, limits) in [
        (
            "compressed_bytes",
            Limits {
                compressed_bytes: exact.compressed_bytes - 1,
                ..exact
            },
        ),
        (
            "extracted_bytes",
            Limits {
                extracted_bytes: exact.extracted_bytes - 1,
                ..exact
            },
        ),
        (
            "file_bytes",
            Limits {
                file_bytes: exact.file_bytes - 1,
                ..exact
            },
        ),
        (
            "entries",
            Limits {
                entries: exact.entries - 1,
                ..exact
            },
        ),
        (
            "manifest_bytes",
            Limits {
                manifest_bytes: exact.manifest_bytes - 1,
                ..exact
            },
        ),
        (
            "artifacts",
            Limits {
                artifacts: 1,
                ..exact
            },
        ),
        (
            "variants_per_artifact",
            Limits {
                variants_per_artifact: 1,
                ..exact
            },
        ),
    ] {
        let error = verify(&archive, limits).unwrap_err().to_string();
        assert!(
            error.contains("release_limit_exceeded") && error.contains(name),
            "{name}: {error}"
        );
        assert!(build(&options, &dir.path().join(format!("{name}.gz")), limits).is_err());
    }
}

fn raw_archive(archive: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(fs::File::open(archive).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

fn gzip(path: &Path, bytes: &[u8]) {
    let mut encoder = flate2::GzBuilder::new()
        .operating_system(255)
        .write(fs::File::create(path).unwrap(), flate2::Compression::best());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap();
}

#[test]
fn format_one_rejects_v2_without_a_compatibility_reader() {
    let (dir, options) = fixture();
    let archive = dir.path().join("release.gz");
    let release = build(&options, &archive, Limits::default()).unwrap();
    assert_eq!(release.manifest().schema, Schema::V1);
    assert_eq!(release.manifest().pack.version, "1.2.3");
    assert_eq!(MEDIA_TYPE, "application/vnd.attune.pack.v1+tar+gzip");
    assert_eq!(
        ENCODING_PROFILE,
        "ustar-miniz_oxide-0.9.1-level9-unicode15.1-v1"
    );
    let mut tar = raw_archive(&archive);
    let old = b"attune.pack.release/v1";
    let start = tar
        .windows(old.len())
        .position(|window| window == old)
        .unwrap();
    tar[start..start + old.len()].copy_from_slice(b"attune.pack.release/v2");
    gzip(&archive, &tar);
    let error = verify(&archive, Limits::default()).unwrap_err();
    assert!(
        error.to_string().contains("invalid release/v1 manifest"),
        "{error:#}"
    );
}

#[test]
fn rejects_noncanonical_containers_and_corruption() {
    let (dir, options) = fixture();
    let good = dir.path().join("good.gz");
    build(&options, &good, Limits::default()).unwrap();
    let original = raw_archive(&good);
    let bad = dir.path().join("bad.gz");
    for (index, value) in [
        (156, b'2'),
        (156, b'1'),
        (156, b'5'),
        (156, b'x'),
        (156, b'g'),
        (156, b'S'),
        (108, b'1'),
        (136, b'1'),
        (257, b'X'),
        (500, 1),
        (148, b'7'),
    ] {
        let mut bytes = original.clone();
        bytes[index] = value;
        gzip(&bad, &bytes);
        assert!(
            verify(&bad, Limits::default()).is_err(),
            "header byte {index}"
        );
    }
    let mut bytes = original.clone();
    bytes.extend([0; 512]);
    gzip(&bad, &bytes);
    assert!(verify(&bad, Limits::default()).is_err());
    gzip(&bad, &original[..original.len() - 512]);
    assert!(verify(&bad, Limits::default()).is_err());
    let mut compressed = fs::read(&good).unwrap();
    compressed.extend(fs::read(&good).unwrap());
    fs::write(&bad, compressed).unwrap();
    assert!(verify(&bad, Limits::default()).is_err());
    let mut compressed = fs::read(&good).unwrap();
    compressed[9] = 3;
    fs::write(&bad, compressed).unwrap();
    assert!(verify(&bad, Limits::default()).is_err());
    let mut bytes = original.clone();
    bytes[512] ^= 1;
    gzip(&bad, &bytes);
    assert!(verify(&bad, Limits::default()).is_err());
    let mut compressed = fs::read(&good).unwrap();
    let n = compressed.len();
    compressed[n - 8] ^= 1;
    fs::write(&bad, compressed).unwrap();
    assert!(verify(&bad, Limits::default()).is_err());
    // Valid checksums do not make dishonest sizes, padding, or duplicate entries valid.
    let first_name = std::str::from_utf8(&original[..100])
        .unwrap()
        .trim_end_matches('\0');
    let first_size =
        u64::from_str_radix(std::str::from_utf8(&original[124..135]).unwrap(), 8).unwrap();
    let mut bytes = original.clone();
    bytes[..512].copy_from_slice(
        &archive::header(first_name, u64::from(u32::MAX), FileMode::Data).unwrap(),
    );
    gzip(&bad, &bytes);
    assert!(verify(&bad, Limits::default())
        .unwrap_err()
        .to_string()
        .contains("file_bytes"));
    let first_end = 512 + first_size.div_ceil(512) as usize * 512;
    let mut bytes = original[..first_end].to_vec();
    bytes.extend_from_slice(&original);
    gzip(&bad, &bytes);
    assert!(verify(&bad, Limits::default()).is_err());
    if first_size % 512 != 0 {
        let mut bytes = original.clone();
        bytes[512 + first_size as usize] = 1;
        gzip(&bad, &bytes);
        assert!(verify(&bad, Limits::default()).is_err());
    }
}

#[test]
fn verifier_rejects_forged_manifests_even_with_valid_canonical_containers() {
    let (dir, options) = fixture();
    let snapshot = tempfile::tempdir().unwrap();
    let original = source::collect(&options, snapshot.path(), Limits::default()).unwrap();
    let bad = dir.path().join("bad.gz");
    for mutation in 0..9 {
        let mut manifest = original.clone();
        match mutation {
            0 => manifest.pack.version = "9.9.9".into(),
            1 => {
                if let Artifact::Jar { component_refs, .. } = &mut manifest.artifacts[0] {
                    component_refs.clear();
                }
            }
            2 => {
                if let Artifact::Jar { component_refs, .. } = &mut manifest.artifacts[0] {
                    component_refs.push("foreign.action".into());
                }
            }
            3 => {
                if let Artifact::Native { variants, .. } = &mut manifest.artifacts[1] {
                    variants[1].target = variants[0].target.clone();
                }
            }
            4 => {
                if let Artifact::Jar { variants, .. } = &mut manifest.artifacts[0] {
                    variants[0].id = "not-portable".into();
                }
            }
            5 => {
                manifest
                    .files
                    .iter_mut()
                    .find(|f| f.path.ends_with("linux-amd64-static"))
                    .unwrap()
                    .mode = FileMode::Data
            }
            6 => manifest.files[0].sha256 = "0".repeat(64),
            7 => manifest.artifacts.reverse(),
            8 => manifest
                .dependencies
                .insert("other".into(), "*".into())
                .map(|_| ())
                .unwrap_or(()),
            _ => unreachable!(),
        }
        archive::write(
            snapshot.path(),
            &manifest,
            &mut fs::File::create(&bad).unwrap(),
            Limits::default(),
        )
        .unwrap();
        assert!(
            verify(&bad, Limits::default()).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn verifier_rejects_noncanonical_json_duplicate_keys_and_self_inventory() {
    let (dir, options) = fixture();
    let good = dir.path().join("good.gz");
    build(&options, &good, Limits::default()).unwrap();
    let original = raw_archive(&good);
    let mut offset = 0;
    let (manifest_offset, manifest_size) = loop {
        let name = std::str::from_utf8(&original[offset..offset + 100])
            .unwrap()
            .trim_end_matches('\0');
        let size = u64::from_str_radix(
            std::str::from_utf8(&original[offset + 124..offset + 135]).unwrap(),
            8,
        )
        .unwrap() as usize;
        if name.ends_with(MANIFEST_PATH) {
            break (offset, size);
        }
        offset += 512 + size.div_ceil(512) * 512;
    };
    let text = std::str::from_utf8(
        &original[manifest_offset + 512..manifest_offset + 512 + manifest_size],
    )
    .unwrap();
    let mut self_inventory: serde_json::Value = serde_json::from_str(text).unwrap();
    self_inventory["files"].as_array_mut().unwrap().push(
        serde_json::json!({"path":MANIFEST_PATH,"mode":"0644","size":0,"sha256":"0".repeat(64)}),
    );
    for text in [
        format!("{text}\n"),
        text.replacen(
            "\"dependencies\":{}",
            "\"dependencies\":{},\"dependencies\":{}",
            1,
        ),
        text.replacen(
            "\"dependencies\":{}",
            "\"dependencies\":{\"other\":\"*\",\"other\":\"*\"}",
            1,
        ),
        text.replacen("\"evidence\":", "\"unknown\":true,\"evidence\":", 1),
        serde_jcs::to_string(&self_inventory).unwrap(),
    ] {
        let mut bytes = original[..manifest_offset].to_vec();
        bytes.extend(
            archive::header(
                &format!("demo/{MANIFEST_PATH}"),
                text.len() as u64,
                FileMode::Data,
            )
            .unwrap(),
        );
        bytes.extend(text.as_bytes());
        bytes.resize(bytes.len().div_ceil(512) * 512, 0);
        bytes.extend_from_slice(
            &original[manifest_offset + 512 + manifest_size.div_ceil(512) * 512..],
        );
        let bad = dir.path().join("bad.gz");
        gzip(&bad, &bytes);
        assert!(verify(&bad, Limits::default()).is_err(), "accepted {text}");
    }
}

#[test]
fn strict_paths_unicode_collisions_and_ustar_split() {
    for path in [
        "/a", "a/./b", "a/../b", "a//b", "a/", "a\\b", "a:b", "a;b", "a*b", "a?b", "a[b", "a]b",
        "a\nb", "e\u{301}",
    ] {
        assert!(paths::validate(path).is_err(), "{path:?}");
    }
    let mut paths = paths::Paths::default();
    paths.insert("demo/Straße").unwrap();
    assert!(paths.insert("demo/STRASSE").is_err());
    let mut paths = paths::Paths::default();
    paths.insert("demo/A/file").unwrap();
    assert!(paths.insert("demo/a/other").is_err());
    assert!(paths.insert("demo/A").is_err());
    let path = format!("{}/{}", "a".repeat(154), "b".repeat(100));
    assert!(paths::validate(&path).is_ok());
    assert!(paths::validate(&(path + "x")).is_err());
    assert!(paths::validate(&"a".repeat(101)).is_err());
}

#[test]
fn strict_manifest_and_tagged_launch_parsing() {
    let (dir, options) = fixture();
    let release = build(&options, &dir.path().join("good.gz"), Limits::default()).unwrap();
    let mut value = serde_json::to_value(release.manifest()).unwrap();
    value["unknown"] = true.into();
    assert!(serde_json::from_value::<Manifest>(value).is_err());
    for value in [
        r#"{"type":"native","artifact":"a","path":"x"}"#,
        r#"{"type":"java_jar","artifact":"a","target":{}}"#,
        r#"{"type":"file","path":"a","path":"b"}"#,
        r#"{"type":"intrinsic","handler":"unknown"}"#,
    ] {
        assert!(
            serde_json::from_str::<LaunchSpec>(value).is_err(),
            "{value}"
        );
    }
    assert!(serde_json::from_str::<ClasspathEntry>(r#"{"path":"x","artifact":"a"}"#).is_err());
    assert!(serde_json::from_str::<Artifact>(r#"{"kind":"jar","id":"a","component_refs":[],"variants":[{"id":"portable","path":"x","target":{}}]}"#).is_err());
}

#[test]
fn rejects_invalid_launches_and_flat_schemas() {
    for content in [
        "ref: demo.native\nrunner_type: native\nentry_point: tool\nlaunch: {type: native, artifact: tool}\n",
        "ref: demo.native\nrunner_type: native\nlaunch: {type: native, artifact: absent}\n",
        "ref: demo.native\nrunner_type: native\nlaunch: {type: native, artifact: app}\n",
        "ref: demo.native\nrunner_type: java\nlaunch: {type: java_class, main_class: Main, classpath: [{path: absent}]}\n",
        "ref: demo.native\nrunner_type: java\nlaunch: {type: java_class, main_class: Main, classpath: [{path: classes}, {path: classes}]}\n",
        "ref: demo.native\nrunner_type: java\nlaunch: {type: java_class, main_class: '-evil', classpath: [{path: classes}]}\n",
        "ref: demo.native\nrunner_type: java\nlaunch: {type: java_jar, artifact: app, jvm_args: ['-cp', '/tmp']}\n",
        "ref: demo.native\nrunner_type: java\nlaunch: {type: java_jar, artifact: app, jvm_args: ['@args']}\n",
        "ref: demo.native\nrunner_type: native\nlaunch: {type: native, artifact: tool}\nparam_schema: {type: object, properties: {}}\n",
    ] {
        let (dir, options) = fixture();
        put(&options.source, "actions/native.yaml", content);
        assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err(), "{content}");
    }
}

#[test]
fn jars_helpers_unused_artifacts_and_reserved_paths_fail_closed() {
    for bytes in [jar(false, false), jar(true, true), b"not a zip".to_vec()] {
        let (dir, options) = fixture();
        put(&options.source, "dist/app.jar", bytes);
        assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
    }
    let (dir, mut options) = fixture();
    options.executables.push("missing".into());
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
    options.executables = vec!["actions/jar.yaml".into()];
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
    options.executables.clear();
    put(&options.source, ".attune/artifacts/undeclared/file", b"bad");
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
}

#[test]
fn development_entry_point_intent_is_explicit_but_v1_paths_are_never_normalized() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    put(&root, "pack.yaml", "ref: legacy\nversion: 1.0.0\n");
    put(
        &root,
        "actions/native.yaml",
        "ref: legacy.native\nrunner_type: native\nentry_point: tool\n",
    );
    put(&root, "actions/tool", b"opaque executable bytes");
    let options = BuildOptions {
        source: root,
        artifacts: vec![],
        executables: vec![],
    };
    let archive = dir.path().join("release.gz");
    let release = build(&options, &archive, Limits::default()).unwrap();
    assert_eq!(
        release
            .manifest()
            .files
            .iter()
            .find(|f| f.path == "actions/tool")
            .unwrap()
            .mode,
        FileMode::Executable
    );
    let mut raw = raw_archive(&archive);
    raw[..100].fill(0);
    let path = b"legacy/./.attune/release-manifest.json";
    raw[..path.len()].copy_from_slice(path);
    gzip(&archive, &raw);
    assert!(verify(&archive, Limits::default())
        .unwrap_err()
        .to_string()
        .contains("unsafe path"));
}

#[cfg(unix)]
#[test]
fn rejects_source_and_artifact_symlinks() {
    let (dir, mut options) = fixture();
    std::os::unix::fs::symlink("README.md", options.source.join("link")).unwrap();
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
    fs::remove_file(options.source.join("link")).unwrap();
    std::os::unix::fs::symlink("dist/app.jar", options.source.join("linked.jar")).unwrap();
    options.artifacts.push(
        format!("other={}/linked.jar", options.source.display())
            .parse()
            .unwrap(),
    );
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
}

#[test]
fn typed_sensors_intrinsics_and_classpath_only_jars() {
    let (dir, options) = fixture();
    put(
        &options.source,
        "triggers/tick.yaml",
        "ref: demo.tick\nparameters: {}\n",
    );
    put(&options.source, "sensors/tick.yaml", "ref: demo.sensor\nrunner_type: NATIVE\nlaunch: {type: native, artifact: tool}\ntrigger_types: [demo.tick]\n");
    // A classpath dependency is a valid JAR without an executable Main-Class.
    fs::remove_file(options.source.join("actions/jar.yaml")).unwrap();
    put(&options.source, "dist/app.jar", jar(false, false));
    let result = build(&options, &dir.path().join("sensor.gz"), Limits::default()).unwrap();
    assert_eq!(
        result.manifest().artifacts[1].component_refs(),
        ["demo.native", "demo.sensor"]
    );
    put(&options.source, "sensors/tick.yaml", "ref: demo.sensor\nlaunch: {type: intrinsic, handler: attune.inquiry/v1}\ntrigger_types: [demo.tick]\n");
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());

    let source = dir.path().join("core");
    put(&source, "pack.yaml", "ref: core\nversion: 1.0.0\n");
    put(
        &source,
        "actions/ask.yaml",
        "ref: core.ask\nlaunch: {type: intrinsic, handler: attune.inquiry/v1}\n",
    );
    let options = BuildOptions {
        source,
        artifacts: vec![],
        executables: vec![],
    };
    assert!(build(
        &options,
        &dir.path().join("intrinsic.gz"),
        Limits::default(),
    )
    .is_err());
    put(
        &options.source,
        "actions/ask.yaml",
        "ref: core.other\nlaunch: {type: intrinsic, handler: attune.inquiry/v1}\n",
    );
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
}

#[test]
fn set_metadata_is_canonicalized_and_current_flat_schemas_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    put(&source, "pack.yaml", "ref: demo\nversion: 1.0.0\ndependencies: ['other@>=1.2,<2']\nrequires:\n  attune: '*'\n  platform_catalog: '>=1'\n  runtimes: [core.shell, core.python, core.shell]\n");
    put(&source, "actions/run.yaml", "ref: demo.run\nrunner_type: SHELL\nlaunch: {type: file, path: scripts/run.sh}\nparam_schema: {message: {type: string}}\nout_schema: {result: {type: boolean}}\n");
    put(&source, "scripts/run.sh", "#!/bin/sh\nexit 0\n");
    let mut options = BuildOptions {
        source,
        artifacts: vec![],
        executables: vec![],
    };
    let result = build(&options, &dir.path().join("release.gz"), Limits::default()).unwrap();
    assert_eq!(
        result.manifest().requires.runtimes,
        ["core.python", "core.shell"]
    );
    assert_eq!(result.manifest().dependencies["other"], ">=1.2, <2");
    options.executables.push("scripts/run.sh".into());
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
    options.executables.clear();
    put(&options.source, "actions/run.yaml", "ref: demo.run\nrunner_type: shell\nlaunch: {type: file, path: scripts/run.sh}\nout_schema: {type: object, properties: {}}\n");
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
}

#[test]
fn evidence_descriptors_are_bound_to_files_and_provenance_fails_closed() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let bytes = br#"{"spdxVersion":"SPDX-2.3"}"#;
    put(&source, "evidence/sbom.json", bytes);
    let digest = hex(&Sha256::digest(bytes));
    let definition = |kind: &str, digest: &str| {
        format!(
        "ref: demo\nversion: 1.0.0\nevidence:\n  {kind}:\n    - path: evidence/sbom.json\n      media_type: application/spdx+json\n      sha256: {digest}\n  {}: []\n",
        if kind == "sboms" { "provenance" } else { "sboms" }
    )
    };
    put(&source, "pack.yaml", definition("sboms", &digest));
    let options = BuildOptions {
        source,
        artifacts: vec![],
        executables: vec![],
    };
    build(&options, &dir.path().join("valid.gz"), Limits::default()).unwrap();
    put(
        &options.source,
        "pack.yaml",
        definition("sboms", &"0".repeat(64)),
    );
    assert!(build(&options, &dir.path().join("bad.gz"), Limits::default()).is_err());
    put(
        &options.source,
        "pack.yaml",
        definition("provenance", &digest),
    );
    let error = build(&options, &dir.path().join("bad.gz"), Limits::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("embedded provenance is unsupported"));
}
