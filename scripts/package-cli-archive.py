#!/usr/bin/env python3
import gzip
from pathlib import Path
import sys
import tarfile
import time
import zipfile


def staged_files(directory: Path) -> list[Path]:
    entries = sorted(directory.rglob("*"), key=lambda path: path.relative_to(directory).as_posix())
    for path in entries:
        if path.is_symlink():
            raise ValueError(f"archive input must not be a symlink: {path}")
    files = [path for path in entries if path.is_file()]
    if not files:
        raise ValueError(f"staging directory is empty: {directory}")
    for path in files:
        if not path.is_file():
            raise ValueError(f"archive input must be a regular file: {path}")
    return files


def archive_mode(path: Path) -> int:
    return 0o755 if path.stat().st_mode & 0o111 else 0o644


def archive_name(path: Path, directory: Path, prefix: str) -> str:
    relative = path.relative_to(directory).as_posix()
    return f"{prefix}/{relative}" if prefix else relative


def write_tar_gz(
    output: Path, files: list[Path], directory: Path, prefix: str, epoch: int
) -> None:
    with output.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                for path in files:
                    info = archive.gettarinfo(
                        str(path), arcname=archive_name(path, directory, prefix)
                    )
                    info.uid = 0
                    info.gid = 0
                    info.uname = "root"
                    info.gname = "root"
                    info.mode = archive_mode(path)
                    info.mtime = epoch
                    with path.open("rb") as source:
                        archive.addfile(info, source)


def write_zip(
    output: Path, files: list[Path], directory: Path, prefix: str, epoch: int
) -> None:
    zip_epoch = max(epoch, 315532800)
    timestamp = time.gmtime(zip_epoch)[:6]
    with zipfile.ZipFile(output, mode="w") as archive:
        for path in files:
            info = zipfile.ZipInfo(
                archive_name(path, directory, prefix), date_time=timestamp
            )
            info.create_system = 3
            info.external_attr = archive_mode(path) << 16
            archive.writestr(
                info,
                path.read_bytes(),
                compress_type=zipfile.ZIP_DEFLATED,
                compresslevel=9,
            )


def main() -> None:
    if len(sys.argv) not in (4, 5):
        raise SystemExit(
            f"Usage: {sys.argv[0]} <output.tar.gz|output.zip> <source-date-epoch> <staging-directory> [archive-prefix]"
        )

    output = Path(sys.argv[1])
    epoch = int(sys.argv[2])
    directory = Path(sys.argv[3])
    prefix = sys.argv[4].strip("/") if len(sys.argv) == 5 else ""
    files = staged_files(directory)

    if output.name.endswith(".tar.gz"):
        write_tar_gz(output, files, directory, prefix, epoch)
    elif output.suffix == ".zip":
        write_zip(output, files, directory, prefix, epoch)
    else:
        raise ValueError(f"unsupported archive format: {output}")


if __name__ == "__main__":
    main()
