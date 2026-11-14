"""Gera o ícone do PostPilot: postpilot-<N>.png (16 a 512 px) e postpilot.ico.

Uso: python3 packaging/icons/make_icon.py  (precisa do Pillow)

Os arquivos gerados ficam no repositório; rode de novo só para mudar o
desenho. Desenha em 4x e reduz, para bordas suaves.
"""

from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
S = 2048  # tela de trabalho (4 x 512)
# Tamanhos dos PNGs (os do tema hicolor do freedesktop).
SIZES = (16, 24, 32, 48, 64, 128, 256, 512)


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def draw() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # Fundo: quadrado arredondado com degradê azul (de cima para baixo).
    top, bottom = (49, 103, 168), (22, 52, 99)
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        gd.line([(0, y), (S, y)], fill=lerp(top, bottom, y / S) + (255,))
    mask = Image.new("L", (S, S), 0)
    margin = 64
    ImageDraw.Draw(mask).rounded_rectangle(
        [margin, margin, S - margin, S - margin], radius=380, fill=255
    )
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    # Cilindro de banco de dados: três discos empilhados.
    cx = S // 2
    w, h = 1000, 300  # largura e altura da elipse
    left, right = cx - w // 2, cx + w // 2
    body_top, body_bottom = 560, 1380
    white = (245, 248, 252, 255)
    shade = (196, 214, 236, 255)
    edge = (22, 52, 99, 255)
    # corpo
    d.rectangle([left, body_top, right, body_bottom], fill=shade)
    d.ellipse([left, body_bottom - h // 2, right, body_bottom + h // 2], fill=shade)
    # faixas entre os discos
    for y in (body_top + 273, body_top + 546):
        d.arc([left, y - h // 2, right, y + h // 2], 0, 180, fill=edge, width=44)
    # tampa
    d.ellipse([left, body_top - h // 2, right, body_top + h // 2], fill=white)
    d.ellipse(
        [left + 90, body_top - h // 2 + 40, right - 90, body_top + h // 2 - 40],
        outline=shade,
        width=28,
    )

    # Destaque: um "play" laranja no canto (executar consulta).
    o = (240, 140, 40, 255)
    px, py, r = 1450, 1450, 330
    d.ellipse([px - r, py - r, px + r, py + r], fill=o)
    d.ellipse([px - r, py - r, px + r, py + r], outline=(255, 255, 255, 255), width=48)
    tri = [(px - 110, py - 170), (px - 110, py + 170), (px + 170, py)]
    d.polygon(tri, fill=(255, 255, 255, 255))
    return img


def main():
    big = draw()
    for n in SIZES:
        big.resize((n, n), Image.LANCZOS).save(HERE / f"postpilot-{n}.png", optimize=True)
    big.resize((256, 256), Image.LANCZOS).save(
        HERE / "postpilot.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )


if __name__ == "__main__":
    main()
