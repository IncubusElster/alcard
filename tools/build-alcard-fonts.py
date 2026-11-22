#!/usr/bin/env python3
"""Deterministic UNSCII HEX -> Alcard console PSF2 and optional pixel-exact TTF.

Use the original user-edited 8x8 regular HEX and original UNSCII-8 bold HEX.
This tool never substitutes or modifies the source glyphs. The only unusual
record in the supplied inputs is U+0000 with 16 zero bytes; it is omitted.
PSF exports keep printable ASCII in same-number glyph slots because Linux VT
uses those slots directly for erase cells and kernel messages.
"""
import argparse
from pathlib import Path
import re
import struct

FONT_NAME = 'Alcard Pixel'
WIDTH = HEIGHT = 8


def read_hex(path):
    glyphs = {}
    for ln, line in enumerate(path.read_text('utf-8-sig').splitlines(), 1):
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        match = re.fullmatch(r'([0-9A-Fa-f]{4,6}):([0-9A-Fa-f]+)', line.strip())
        if not match:
            raise ValueError(f'{path}:{ln}: expected UnicodeHEX:BITMAPHEX')
        cp = int(match.group(1), 16)
        rows = bytes.fromhex(match.group(2))
        if cp == 0 and rows == bytes(16):
            # UNSCII retains a 16-row null bitmap; not a displayable glyph.
            continue
        if len(rows) != 8:
            raise ValueError(f'{path}:{ln}: glyph U+{cp:04X} is not 8x8')
        if cp in glyphs:
            raise ValueError(f'{path}:{ln}: repeated U+{cp:04X}')
        glyphs[cp] = rows
    return glyphs


def unique_order(*groups):
    seen = set()
    for group in groups:
        for cp in group:
            if cp not in seen:
                seen.add(cp)
                yield cp


def coverage(glyphs, full=False):
    available = set(glyphs)
    ascii_ = range(0x20, 0x7f)
    russian = [ord(c) for c in "АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдеёжзийклмнопрстуфхцчшщъыьэюя"]
    pinyin = [ord(c) for c in "āáǎàēéěèīíǐìōóǒòūúǔùǖǘǚǜüńňǹ"]
    spanish = [ord(c) for c in "áéíóúüñÁÉÍÓÚÜÑ¡¿"]
    ru_extended = [0x490, 0x491, 0x4D0, 0x4D1]
    latin_extra = [0xA0, 0xA1, 0xA9, 0xAE, 0xB0, 0xB1, 0xB7, 0xBF, 0xC1,
                   0xC9, 0xCD, 0xD1, 0xD3, 0xDA, 0xDC, 0xDF, 0xE0, 0xE1,
                   0xE9, 0xED, 0xF1, 0xF3, 0xFA, 0xFC, 0xC0, 0xC2, 0xC7,
                   0xD6, 0xD9, 0xE2, 0xE7, 0xF6, 0xF9]
    box_core = [0x2500,0x2502,0x250C,0x2510,0x2514,0x2518,0x251C,0x2524,
                0x252C,0x2534,0x253C,0x2550,0x2551,0x2554,0x2557,0x255A,
                0x255D,0x2560,0x2563,0x2566,0x2569,0x256C,0x2571,0x2572,
                0x2573,0x2574,0x2575,0x2576,0x2577]
    blocks_core = [0x2580,0x2584,0x2588,0x258C,0x2590,0x2591,0x2592,
                   0x2593,0x2594,0x2595,0x2581,0x2582,0x2583,0x2585,0x2586,
                   0x2587]
    arrows = list(range(0x2190, 0x21a0))
    ui_symbol = [0x00AB,0x00BB,0x00B5,0x00D7,0x00F7,0x2022,0x2026,0x2013,
                 0x2014,0x2018,0x2019,0x201C,0x201D,0x2010,0x2011,0x2020,
                 0x20AC,0x2116,0x2122,0x2212,0x221A,0x221E,0x2260,0x2264,
                 0x2265,0x25A0,0x25A1,0x25B2,0x25BC,0x25C6,0x25CB,0x25CF,
                 0x2605,0x2606,0x2713,0x2717,0x2302,0x2320,0x2321]
    if full:
        preferred = unique_order(
            ascii_,russian,spanish,pinyin,range(0xA0,0x100),ru_extended,
            range(0x2500,0x2580),range(0x2580,0x25A0),arrows,
            ui_symbol,range(0x390,0x400),range(0x2000,0x2100),
            range(0x2100,0x2300),sorted(available))
        cap=512
    else:
        preferred=unique_order(ascii_,russian,spanish,pinyin,box_core,
                               blocks_core,arrows,ui_symbol,latin_extra,ru_extended,range(0xA0,0x100),
                               sorted(available))
        cap=256
    picked=[]
    for cp in preferred:
        if cp in available:
            picked.append(cp)
            if len(picked)==cap:break
    if len(picked)!=cap:
        raise ValueError(f'Expected {cap} glyphs, got {len(picked)}')
    return picked


def console_glyph_order(sequence):
    # Linux VT bypasses the Unicode map for erase (' ') and printk bytes.
    # Reserve only the actual printable ASCII positions, then put every other
    # selected codepoint in a free slot. This is a permutation: no language
    # coverage, source bitmap, or 256/512-glyph capacity is sacrificed.
    ascii_ = range(0x20, 0x7f)
    if len(sequence) != len(set(sequence)) or not set(ascii_) <= set(sequence):
        raise ValueError('Console PSF requires unique glyphs and all printable ASCII')
    if len(sequence) < 0x7f:
        raise ValueError('Console PSF has no room for the printable ASCII slots')
    remaining = iter(cp for cp in sequence if cp not in ascii_)
    return [slot if slot in ascii_ else next(remaining) for slot in range(len(sequence))]


def write_psf2(path, glyphs, sequence):
    sequence = console_glyph_order(sequence)
    # PSF2_MAGIC, version, header_size, flags, length, charsize, height, width
    head=struct.pack('<8I',0x864AB572,0,32,1,len(sequence),8,8,8)
    bitmaps=b''.join(glyphs[cp] for cp in sequence)
    # Each codepoint gets UTF-8 glyph mapping; map U+FFFD to question mark.
    unicode_table=b''
    for cp in sequence:
        unicode_table+=chr(cp).encode('utf-8')
        if cp==0x3f:
            unicode_table+='\uFFFD'.encode('utf-8')
        unicode_table+=b'\xff'
    path.write_bytes(head+bitmaps+unicode_table)
    return len(sequence)


def draw_glyph(rows):
    from fontTools.pens.ttGlyphPen import TTGlyphPen
    pen=TTGlyphPen(None)
    for y, bits in enumerate(rows):
        for x in range(8):
            if bits & (0x80 >> x):
                left=x*128
                top=896-y*128
                pen.moveTo((left,top-128))
                pen.lineTo((left+128,top-128))
                pen.lineTo((left+128,top))
                pen.lineTo((left,top))
                pen.closePath()
    return pen.glyph()


def write_ttf(path,glyphs,style):
    from fontTools.fontBuilder import FontBuilder
    fb=FontBuilder(1024,isTTF=True)
    cps=sorted(glyphs)
    glyphnames={cp:f'u{cp:06X}' for cp in cps}
    order=['.notdef']+[glyphnames[cp] for cp in cps]
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap({cp:glyphnames[cp] for cp in cps})
    allglyphs={'.notdef':draw_glyph(bytes.fromhex('7E424A5252427E00'))}
    for cp in cps:
        allglyphs[glyphnames[cp]]=draw_glyph(glyphs[cp])
    fb.setupGlyf(allglyphs)
    fb.setupHorizontalMetrics({name:(1024,0) for name in order})
    fb.setupHorizontalHeader(ascent=896,descent=-128)
    fb.setupNameTable({'familyName':FONT_NAME,'styleName':style,
                       'uniqueFontIdentifier':f'Alcard Pixel {style} 0.1',
                       'fullName':f'{FONT_NAME} {style}',
                       'psName':f'AlcardPixel-{style}',
                       'version':'Version 0.1.0',
                       'designer':'UNSCII by Viznut; regular glyph modifications by Alcard contributor'})
    fb.setupOS2(sTypoAscender=896,sTypoDescender=-128,usWinAscent=896,
                usWinDescent=128,usWeightClass=(700 if style=='Bold' else 400),
                sTypoLineGap=0,sxHeight=640,sCapHeight=768)
    fb.setupPost(isFixedPitch=1)
    fb.setupMaxp()
    fb.save(path)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--regular',type=Path,required=True)
    parser.add_argument('--bold',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--ttf',action='store_true',help='also generate TTF using fontTools')
    args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    regular=read_hex(args.regular)
    bold=read_hex(args.bold)
    if set(regular)!=set(bold):
        raise ValueError('Styles have different Unicode coverage; do not silently merge')
    for style,glyphs in [('Regular',regular),('Bold',bold)]:
        for size,cap in [('core',256),('extended',512)]:
            path=args.output/f'AlcardPixel-{style}-{size}-8x8.psf'
            n=write_psf2(path,glyphs,coverage(glyphs,full=cap==512))
            print(path.name, 'glyphs:', n,'bytes:',path.stat().st_size)
        if args.ttf:
            path=args.output/f'AlcardPixel-{style}.ttf'
            write_ttf(path,glyphs,style)
            print(path.name,'Unicode glyphs:',len(glyphs),'bytes:',path.stat().st_size)
    print('Input glyphs:',len(regular),'Differences:',sum(regular[x]!=bold[x] for x in regular))

if __name__=='__main__':
    main()
