"""Download the official Tor Expert Bundle for a platform and unpack it.

    python packaging/fetch_tor.py windows-x86_64 dist/tor
    python packaging/fetch_tor.py macos-aarch64  dist/tor

Picks the newest stable release on dist.torproject.org that has an expert bundle for
the platform, and checks the download against the release's sha256sums file.
The result is the bundle's own layout: <out>/tor/tor[.exe] and <out>/data/geoip.
"""
import hashlib
import io
import re
import shutil
import sys
import tarfile
import urllib.request
from pathlib import Path

BASE = "https://dist.torproject.org/torbrowser/"
UA = {"User-Agent": "geo-unlocker-build"}


def get(url: str) -> bytes:
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=120) as r:
        return r.read()


def versions():
    html = get(BASE).decode("utf-8", "replace")
    found = set(re.findall(r'href="(\d+(?:\.\d+)+)/"', html))  # stable only: no "a" (alpha)
    return sorted(found, key=lambda v: [int(x) for x in v.split(".")], reverse=True)


def main(platform: str, out: str) -> None:
    for ver in versions():
        name = f"tor-expert-bundle-{platform}-{ver}.tar.gz"
        try:
            sums = get(f"{BASE}{ver}/sha256sums-signed-build.txt").decode()
        except Exception:
            continue
        expected = next((l.split()[0] for l in sums.splitlines() if l.strip().endswith(name)), None)
        if not expected:
            continue
        print(f"Tor Expert Bundle {ver} for {platform}")
        data = get(f"{BASE}{ver}/{name}")
        actual = hashlib.sha256(data).hexdigest()
        if actual != expected:
            sys.exit(f"checksum mismatch for {name}: {actual} != {expected}")
        dest = Path(out)
        if dest.exists():
            shutil.rmtree(dest)
        dest.mkdir(parents=True)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
            tar.extractall(dest, filter="data")
        # Geo Unlocker doesn't use pluggable transports (bridges); keep tor, geoip and licenses.
        for extra in ["tor/pluggable_transports", "tor/tor-gencert", "tor/tor-gencert.exe", "debug"]:
            path = dest / extra
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists():
                path.unlink()
        (dest / "VERSION.txt").write_text(f"Tor Expert Bundle {ver} ({name}, sha256 {actual})\n")
        print(f"unpacked to {dest}")
        return
    sys.exit(f"no expert bundle found for {platform}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
