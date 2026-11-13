"""Generate the app icon (PNG + Windows ICO) without external libraries.

    python packaging/make_icons.py

Writes packaging/icons/geo-unlocker-{16,32,48,128,256,512,1024}.png and geo-unlocker.ico.
The design matches the in-app window icon: a purple disc with a white globe.
"""
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).parent / "icons"


def render(n: int) -> bytes:
    """RGBA pixels, 4x supersampled for smooth edges."""
    ss, c = 4, n / 2
    px = bytearray()
    for y in range(n):
        for x in range(n):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(ss):
                for sx in range(ss):
                    fx, fy = x + (sx + 0.5) / ss, y + (sy + 0.5) / ss
                    dx, dy = (fx - c) / c, (fy - c) / c
                    d = (dx * dx + dy * dy) ** 0.5
                    if d > 0.98:
                        continue
                    w = 0.055  # stroke width (fraction of radius)
                    ring = abs(d - 0.60) < w
                    cross = (abs(dx) < w or abs(dy) < w) and d < 0.60 + w
                    ell = abs(((dx / 0.28) ** 2 + (dy / 0.60) ** 2) ** 0.5 - 1) < w * 2.2 and d < 0.62
                    if ring or cross or ell:
                        col = (255, 255, 255)
                    else:
                        t = fy / n
                        col = (int(155 - 50 * t), int(133 - 70 * t), 255)
                    acc[0] += col[0]; acc[1] += col[1]; acc[2] += col[2]; acc[3] += 255
            k = ss * ss
            a = acc[3] / k
            if a == 0:
                px += b"\0\0\0\0"
            else:
                px += bytes([int(acc[0] / (acc[3] / 255)), int(acc[1] / (acc[3] / 255)), int(acc[2] / (acc[3] / 255)), int(a)])
    return bytes(px)


def png(n: int, rgba: bytes) -> bytes:
    raw = b"".join(b"\0" + rgba[y * n * 4:(y + 1) * n * 4] for y in range(n))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def ico(images: dict) -> bytes:
    sizes = sorted(images)
    header = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries, blobs = b"", b""
    for s in sizes:
        data = images[s]
        entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
        blobs += data
    return header + entries + blobs


def main() -> None:
    OUT.mkdir(exist_ok=True)
    pngs = {}
    for n in (16, 32, 48, 128, 256, 512, 1024):
        pngs[n] = png(n, render(n))
        (OUT / f"geo-unlocker-{n}.png").write_bytes(pngs[n])
    (OUT / "geo-unlocker.ico").write_bytes(ico({n: pngs[n] for n in (16, 32, 48, 256)}))
    print("icons written to", OUT)


if __name__ == "__main__":
    main()
