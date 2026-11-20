#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = [
#   "httpx>=0.27,<1",
#   "warcio>=1.7,<2",
# ]
# ///
"""Download a deterministic Common Crawl WARC sample and export HTML records.

Run from anywhere:
    uv run scripts/download_commoncrawl.py

The project root is inferred from this file's location. Data is written to:
    data/commoncrawl/warc/*.warc.gz
    data/commoncrawl/html/*.html
    data/commoncrawl/html/manifest.jsonl

The WARC is streamed directly by warcio; it is not expanded into a separate
multi-gigabyte .warc file first. HTTP Content-Encoding and chunked transfer
encoding are decoded by warcio's content_stream(), but HTML bytes are not
charset-decoded, normalized, or re-serialized.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import httpx
from warcio.archiveiterator import ArchiveIterator

PROJECT_ROOT = Path(__file__).resolve().parents[1]
DATA_ROOT = PROJECT_ROOT / "data" / "commoncrawl"
WARC_DIR = DATA_ROOT / "warc"
HTML_DIR = DATA_ROOT / "html"


# Fixed sample: one known WARC object from CC-MAIN-2026-43.
# Keeping the URL and expected byte size fixed makes download selection
# reproducible and catches truncated or unexpectedly changed downloads.
@dataclass(frozen=True)
class WarcFile:
    url: str
    size: int

    @property
    def filename(self) -> str:
        return self.url.rsplit("/", 1)[-1]


WARC_FILES = (
    WarcFile(
        url=(
            "https://data.commoncrawl.org/crawl-data/CC-MAIN-2026-43/"
            "segments/1791170053823.21/warc/"
            "CC-MAIN-20261005105623-20261005135623-00993.warc.gz"
        ),
        size=958_726_164,
    ),
)

CHUNK_SIZE = 1024 * 1024
DOWNLOAD_RETRIES = 6
HTTP_TIMEOUT = httpx.Timeout(120.0, connect=30.0)


def format_bytes(value: int) -> str:
    size = float(value)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if size < 1024 or unit == "TiB":
            return f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} TiB"


def find_existing_download(destination: Path, filename: str) -> Path | None:
    """Reuse the old download from the project root, if present."""
    candidates = (
        destination,
        PROJECT_ROOT / filename,
    )
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


def download_one(client: httpx.Client, item: WarcFile, destination: Path) -> Path:
    destination.parent.mkdir(parents=True, exist_ok=True)

    existing = find_existing_download(destination, item.filename)
    if existing is not None and existing.stat().st_size == item.size:
        if existing != destination:
            print(f"[reuse] Moving existing download into {destination}")
            os.replace(existing, destination)
        else:
            print(f"[skip] Already downloaded: {destination.name}")
        return destination

    if existing is not None and existing != destination:
        actual = existing.stat().st_size
        print(
            f"[warn] Found {existing} ({format_bytes(actual)}), but expected "
            f"{format_bytes(item.size)}. It will not be reused.",
            file=sys.stderr,
        )

    partial = destination.with_name(destination.name + ".part")
    if partial.exists() and partial.stat().st_size > item.size:
        print(f"[warn] Removing oversized partial download: {partial}")
        partial.unlink()

    for attempt in range(1, DOWNLOAD_RETRIES + 1):
        offset = partial.stat().st_size if partial.exists() else 0
        headers = {
            "Accept-Encoding": "identity",
            "User-Agent": "simdhtml-commoncrawl-dataset/1.0",
        }
        if offset:
            headers["Range"] = f"bytes={offset}-"

        mode = "ab" if offset else "wb"
        try:
            print(
                f"[download] {item.filename}: {format_bytes(offset)} / "
                f"{format_bytes(item.size)} (attempt {attempt}/{DOWNLOAD_RETRIES})"
            )
            with client.stream("GET", item.url, headers=headers) as response:
                if offset and response.status_code == 206:
                    content_range = response.headers.get("content-range", "")
                    match = re.fullmatch(r"bytes (\d+)-(\d+)/(\d+|\*)", content_range)
                    if (
                        not match
                        or int(match.group(1)) != offset
                        or (match.group(3) != "*" and int(match.group(3)) != item.size)
                    ):
                        raise RuntimeError(
                            f"Unexpected Content-Range while resuming: {content_range!r}"
                        )
                elif response.status_code == 200:
                    # Server ignored Range; restart from byte zero.
                    offset = 0
                    mode = "wb"
                elif response.status_code == 416:
                    if partial.exists() and partial.stat().st_size == item.size:
                        break
                    raise RuntimeError("Server rejected the resume range (HTTP 416)")
                else:
                    response.raise_for_status()

                written = offset
                last_report = written
                with partial.open(mode) as output:
                    for chunk in response.iter_raw(chunk_size=CHUNK_SIZE):
                        if not chunk:
                            continue
                        output.write(chunk)
                        written += len(chunk)
                        if written - last_report >= 64 * 1024 * 1024:
                            print(
                                f"  {format_bytes(written)} / {format_bytes(item.size)}"
                            )
                            last_report = written

            actual = partial.stat().st_size
            if actual == item.size:
                os.replace(partial, destination)
                print(f"[done] {destination} ({format_bytes(actual)})")
                return destination
            if actual > item.size:
                raise RuntimeError(
                    f"Downloaded {format_bytes(actual)}, exceeding expected "
                    f"{format_bytes(item.size)}"
                )
            print(
                f"[incomplete] Got {format_bytes(actual)} / "
                f"{format_bytes(item.size)}; will resume."
            )
        except (httpx.HTTPError, OSError, RuntimeError) as error:
            if attempt == DOWNLOAD_RETRIES:
                raise RuntimeError(
                    f"Download failed for {item.url}: {error}"
                ) from error
            print(f"[retry] {error}", file=sys.stderr)

        if attempt < DOWNLOAD_RETRIES:
            time.sleep(min(2 ** (attempt - 1), 20))

    # A 416 response can indicate that a prior attempt completed the file.
    if partial.exists() and partial.stat().st_size == item.size:
        os.replace(partial, destination)
        print(f"[done] {destination} ({format_bytes(item.size)})")
        return destination

    raise RuntimeError(f"Could not finish downloading {item.filename}")


def extract_html(warc_path: Path, output_dir: Path, force: bool = False) -> None:
    success_marker = output_dir / "_SUCCESS"
    manifest_path = output_dir / "manifest.jsonl"
    if success_marker.is_file() and manifest_path.is_file() and not force:
        print(f"[skip] HTML dataset already extracted: {output_dir}")
        print("       Pass --force-extract to rebuild it.")
        return

    staging_dir = output_dir.with_name(output_dir.name + ".tmp")
    if staging_dir.exists():
        shutil.rmtree(staging_dir)
    staging_dir.mkdir(parents=True)

    record_count = 0
    total_bytes = 0
    response_count = 0
    html_candidates = 0
    html_count = 0

    print(f"[extract] Reading {warc_path.name}")
    try:
        with (
            warc_path.open("rb") as archive,
            (staging_dir / "manifest.jsonl").open(
                "w", encoding="utf-8", newline="\n"
            ) as manifest,
        ):
            for record in ArchiveIterator(archive):
                record_count += 1
                if record.rec_type != "response":
                    continue

                response_count += 1
                headers = record.http_headers
                if headers is None:
                    continue

                status = str(headers.get_statuscode())
                if status != "200":
                    continue

                raw_content_type = headers.get_header("Content-Type") or ""
                content_type = raw_content_type.split(";", 1)[0].strip().lower()
                if content_type not in {"text/html", "application/xhtml+xml"}:
                    continue

                html_candidates += 1
                filename = f"{html_count:06d}.html"
                output_path = staging_dir / filename
                extracted_size = 0

                # content_stream() removes HTTP transfer/content encodings.
                # It does not perform HTML charset decoding or DOM serialization.
                body = record.content_stream()
                with output_path.open("wb") as output:
                    while True:
                        chunk = body.read(CHUNK_SIZE)
                        if not chunk:
                            break
                        output.write(chunk)
                        extracted_size += len(chunk)

                if extracted_size == 0:
                    output_path.unlink(missing_ok=True)
                    continue

                html_count += 1
                item = {
                    "file": filename,
                    "url": record.rec_headers.get_header("WARC-Target-URI"),
                    "date": record.rec_headers.get_header("WARC-Date"),
                    "warc_record_id": record.rec_headers.get_header("WARC-Record-ID"),
                    "http_status": status,
                    "content_type": raw_content_type,
                    "size": extracted_size,
                    "warc_truncated": record.rec_headers.get_header("WARC-Truncated"),
                }
                manifest.write(json.dumps(item, ensure_ascii=False) + "\n")
                total_bytes += extracted_size

                if html_count % 500 == 0:
                    print(
                        f"  extracted {html_count} HTML files, "
                        f"{format_bytes(total_bytes)}"
                    )

        summary = {
            "source_warc": warc_path.name,
            "records_seen": record_count,
            "response_records": response_count,
            "html_candidates": html_candidates,
            "html_files": html_count,
            "html_bytes": total_bytes,
        }
        (staging_dir / "_SUCCESS").write_text(
            json.dumps(summary, indent=2) + "\n", encoding="utf-8"
        )

        if output_dir.exists():
            shutil.rmtree(output_dir)
        os.replace(staging_dir, output_dir)

        print(f"[done] Extracted {html_count} HTML files")
        print(f"       Total HTML: {format_bytes(total_bytes)}")
        print(f"       Manifest:  {output_dir / 'manifest.jsonl'}")
    except Exception:
        # Keep the staging directory for inspection after a failure.
        print(f"[error] Extraction incomplete; partial files remain in {staging_dir}")
        raise


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Download a fixed Common Crawl WARC sample and export HTML files."
    )
    parser.add_argument(
        "--force-extract",
        action="store_true",
        help="rebuild the HTML directory even if a successful extraction already exists",
    )
    args = parser.parse_args()

    WARC_DIR.mkdir(parents=True, exist_ok=True)
    HTML_DIR.parent.mkdir(parents=True, exist_ok=True)

    try:
        with httpx.Client(follow_redirects=True, timeout=HTTP_TIMEOUT) as client:
            for item in WARC_FILES:
                warc_path = download_one(client, item, WARC_DIR / item.filename)
                extract_html(warc_path, HTML_DIR, force=args.force_extract)
    except KeyboardInterrupt:
        print(
            "\n[interrupted] Partial download is kept and can be resumed.",
            file=sys.stderr,
        )
        return 130
    except Exception as error:
        print(f"[error] {error}", file=sys.stderr)
        return 1

    print(f"\nDataset ready: {DATA_ROOT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
