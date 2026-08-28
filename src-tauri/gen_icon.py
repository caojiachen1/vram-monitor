"""Generate src-tauri/icons/icon.ico without external deps: PNG-encoded ICO entries."""
import io, struct, zlib, os

def png_chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

def encode_png(size, rgba):
    raw = b"".join(b"\x00" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", ihdr)
            + png_chunk(b"IDAT", zlib.compress(raw, 9)) + png_chunk(b"IEND", b""))

def draw(size):
    px = bytearray(size * size * 4)
    r = size * 0.22  # corner radius
    pad = size * 0.16
    bar_h = size * 0.12
    fill_frac = 0.62
    for y in range(size):
        for x in range(size):
            # rounded-square mask
            cx = min(x, size - 1 - x)
            cy = min(y, size - 1 - y)
            inside = True
            if cx < r and cy < r and ((r - cx) ** 2 + (r - cy) ** 2) > r * r:
                inside = False
            if not inside:
                continue
            i = (y * size + x) * 4
            bar_top = size * 0.30
            bar_bot = bar_top + bar_h
            if bar_top <= y < bar_bot and pad <= x < size - pad:
                frac = (x - pad) / (size - 2 * pad)
                if frac < fill_frac:
                    px[i:i + 4] = (118, 185, 0, 255)   # accent green
                else:
                    px[i:i + 4] = (42, 46, 55, 255)    # track
            elif bar_bot + size * 0.06 <= y < bar_bot + size * 0.06 + bar_h * 0.55 and pad <= x < pad + (size - 2 * pad) * 0.45:
                px[i:i + 4] = (118, 185, 0, 255)       # small green chip
            else:
                px[i:i + 4] = (30, 33, 40, 255)        # card bg
    return bytes(px)

def main():
    out_dir = os.path.join(os.path.dirname(__file__), "icons")
    os.makedirs(out_dir, exist_ok=True)
    entries = []
    for s in (32, 256):
        png = encode_png(s, draw(s))
        entries.append((s, png))
    ico = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    for s, png in entries:
        ico += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
    for _, png in entries:
        ico += png
    with open(os.path.join(out_dir, "icon.ico"), "wb") as f:
        f.write(ico)
    print("icon.ico written:", len(ico), "bytes")

if __name__ == "__main__":
    main()
