# Regenerates the original stamp + toolbar SVGs: python3 scripts/gen-icons.py .
import math, os, sys
root = sys.argv[1]
stamps = os.path.join(root, "icons", "stamps")
hdr = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
def S(name, body):
    with open(os.path.join(stamps, name + ".svg"), "w") as f:
        f.write(hdr + body + "</svg>\n")

arrow = '<path d="M32 6 L54 30 H40 V58 H24 V30 H10 Z"/>'
S("arrow-up", arrow)
S("arrow-down", f'<g transform="rotate(180 32 32)">{arrow}</g>')
S("arrow-left", f'<g transform="rotate(-90 32 32)">{arrow}</g>')
S("arrow-right", f'<g transform="rotate(90 32 32)">{arrow}</g>')
S("arrow-up-right", f'<g transform="rotate(45 32 32)">{arrow}</g>')
S("arrow-down-right", f'<g transform="rotate(135 32 32)">{arrow}</g>')
S("arrow-down-left", f'<g transform="rotate(225 32 32)">{arrow}</g>')
S("arrow-up-left", f'<g transform="rotate(315 32 32)">{arrow}</g>')
S("caret-right", '<path d="M18 6 L50 32 L18 58 Z"/>')
S("circle", '<circle cx="32" cy="32" r="26"/>')
S("circle-ring", '<path fill-rule="evenodd" d="M32 4a28 28 0 1 0 0 56a28 28 0 1 0 0-56zM32 12a20 20 0 1 1 0 40a20 20 0 1 1 0-40z"/>')
S("square", '<rect x="6" y="6" width="52" height="52" rx="4"/>')
S("square-frame", '<path fill-rule="evenodd" d="M4 4h56v56H4zM12 12v40h40V12z"/>')
S("triangle", '<path d="M32 6 L60 56 H4 Z"/>')
pts = []
for i in range(10):
    r = 28 if i % 2 == 0 else 12
    a = -math.pi / 2 + i * math.pi / 5
    pts.append(f"{32 + r * math.cos(a):.2f},{32 + r * math.sin(a):.2f}")
S("star", f'<polygon points="{" ".join(pts)}"/>')
S("heart", '<path d="M32 58 C8 40 4 28 4 20 A14 14 0 0 1 32 16 A14 14 0 0 1 60 20 C60 28 56 40 32 58 Z"/>')
S("check-mark", '<path d="M8 34 L16 26 L26 36 L48 12 L56 20 L26 52 Z"/>')
S("times-mark", '<path d="M12 4 L32 24 L52 4 L60 12 L40 32 L60 52 L52 60 L32 40 L12 60 L4 52 L24 32 L4 12 Z"/>')
S("question-mark", '<path d="M22 22 A10 10 0 1 1 36 31 C33 33 32 35 32 39 H24 C24 32 27 29 31 27 C33 26 34 24 34 22 A2 2 0 0 0 30 22 Z"/><rect x="24" y="44" width="10" height="10" rx="2"/>')
S("exclamation-mark", '<path d="M24 6 H40 L37 38 H27 Z"/><rect x="24" y="44" width="16" height="12" rx="3"/>')
S("plus-sign", '<path d="M26 6h12v20h20v12H38v20H26V38H6V26h20z"/>')
S("minus-sign", '<rect x="6" y="26" width="52" height="12" rx="2"/>')
S("equals-sign", '<rect x="6" y="14" width="52" height="12" rx="2"/><rect x="6" y="38" width="52" height="12" rx="2"/>')
S("link-pin", '<path fill-rule="evenodd" d="M32 4a18 18 0 0 0-18 18c0 14 18 38 18 38s18-24 18-38A18 18 0 0 0 32 4zm0 10a8 8 0 1 1 0 16a8 8 0 1 1 0-16z"/>')

tb = 'xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"'
def T(name, body):
    with open(os.path.join(root, "icons", name + ".svg"), "w") as f:
        f.write(f'<svg {tb}>{body}</svg>\n')
T("crop", '<path d="M6 2v14a2 2 0 0 0 2 2h14"/><path d="M2 6h14a2 2 0 0 1 2 2v14"/>')
T("eyedropper", '<path d="M20 4a2.8 2.8 0 0 0-4 0l-2.5 2.5-1-1-2 2 1 1L3 16.5V21h4.5l7.5-7.5 1 1 2-2-1-1L20 8a2.8 2.8 0 0 0 0-4z"/>')
T("step-list", '<path d="M10 6h11M10 12h11M10 18h11"/><path d="M4 5l1.5-1v4M3.5 11.5h3L3.5 14.5h3M3.5 17h3v3h-3"/>')
T("marquee", '<path d="M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3"/>')
