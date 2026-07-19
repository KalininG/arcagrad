import urllib.request, re, os

UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
WEB = os.path.expanduser("~/arcagrad/web")
FONTS = os.path.join(WEB, "static/fonts")
os.makedirs(FONTS, exist_ok=True)

families = [
    ("shippori-mincho", "Shippori+Mincho:wght@500;700"),
    ("inter", "Inter:wght@400;500;600"),
    ("jetbrains-mono", "JetBrains+Mono:wght@400;500"),
]
KEEP = {"latin", "latin-ext"}

def get(url):
    return urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": UA})).read()

faces = []
for slug, fam in families:
    css = get(f"https://fonts.googleapis.com/css2?family={fam}&display=swap").decode()
    for subset, block in re.findall(r"/\*\s*([\w-]+)\s*\*/\s*(@font-face\s*\{[^}]*\})", css):
        if subset not in KEEP:
            continue
        name = re.search(r"font-family:\s*\x27([^\x27]+)\x27", block).group(1)
        weight = re.search(r"font-weight:\s*(\d+)", block).group(1)
        style = re.search(r"font-style:\s*(\w+)", block).group(1)
        urange = re.search(r"unicode-range:\s*([^;]+);", block).group(1)
        woff2 = re.search(r"src:\s*url\(([^)]+)\)\s*format\(\x27woff2\x27\)", block).group(1)
        fname = f"{slug}-{weight}-{subset}.woff2"
        data = get(woff2)
        open(os.path.join(FONTS, fname), "wb").write(data)
        faces.append((name, style, weight, fname, urange))
        print(f"  {fname}  {len(data)//1024} KB")

out = ["/* Self-hosted fonts, vendored from Google Fonts (latin + latin-ext subsets).",
       "   CJK glyphs fall back to the serif/system stack. Regenerate with",
       "   scripts/fetch-fonts.py. No runtime requests to Google. */", ""]
for name, style, weight, fname, urange in faces:
    out.append("@font-face {")
    out.append(f"  font-family: \x27{name}\x27;")
    out.append(f"  font-style: {style};")
    out.append(f"  font-weight: {weight};")
    out.append("  font-display: optional;")
    out.append(f"  src: url(\x27/fonts/{fname}\x27) format(\x27woff2\x27);")
    out.append(f"  unicode-range: {urange};")
    out.append("}")
open(os.path.join(WEB, "src/fonts.css"), "w").write("\n".join(out) + "\n")
print(f"wrote src/fonts.css with {len(faces)} @font-face blocks")
