//! Renders a type specimen of the built-in face on three grounds.
//! Usage: cargo run -p ok-gfx --example specimen -- out.png

use ok_gfx::{encode_png, BitmapFont, Frame, Pen, Rect, Rgb};

const GALLANT: &str = include_str!("../../../assets/fonts/gallant/gallant.hex");

fn text(frame: &mut Frame, font: &BitmapFont, x: i32, y: i32, s: &str, pen: Pen) {
    let cell = font.cell().0 as i32 * pen.scale as i32;
    let mut cx = x;
    for ch in s.chars() {
        if ('\u{0300}'..='\u{036F}').contains(&ch) {
            font.draw(frame, ch, cx - cell, y, pen);
        } else {
            cx += font.draw(frame, ch, cx, y, pen);
        }
    }
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: specimen out.png");
    let font = BitmapFont::from_hex("Gallant", GALLANT).expect("gallant.hex");
    let (cw, ch) = (font.cell().0 as i32, font.cell().1 as i32);
    let mut frame = Frame::new(1920, 1080);

    // Provisional values, only to see the face on a ground. The real tokens
    // are sampled and sourced in the theme files.
    let bands = [
        ("manila folder", Rgb(0xE6, 0xD2, 0xA8), Rgb(0x2A, 0x24, 0x1E), Rgb(0x7A, 0x66, 0x4A), Rgb(0x8C, 0x3B, 0x1E)),
        ("console white", Rgb(0xF6, 0xF5, 0xEF), Rgb(0x14, 0x14, 0x12), Rgb(0x6C, 0x6B, 0x64), Rgb(0x1F, 0x4E, 0x8C)),
        ("amber phosphor", Rgb(0x1B, 0x15, 0x0C), Rgb(0xFF, 0xB0, 0x2E), Rgb(0xA8, 0x72, 0x1C), Rgb(0xFF, 0xD9, 0x8A)),
    ];
    let lines = [
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "abcdefghijklmnopqrstuvwxyz",
        "0123456789  .,;:!?  ()[]{}  @#$%&*",
        "",
        "\u{201C}Curly quotes\u{201D} and \u{2018}single\u{2019} \u{2014} em, \u{2013} en, \u{2026}",
        "\u{00BF}Qu\u{00E9} hay de nuevo? \u{00A1}A\u{00F1}o, ni\u{00F1}o, ping\u{00FC}ino, acci\u{00F3}n!",
        "mitsqanaqan\u{0313}, Venture\u{00F1}o Chumash",
        "",
        "\u{250C}\u{2500}\u{2500}\u{2500}\u{2500}\u{252C}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}  \u{2591}\u{2592}\u{2593}\u{2588}  \u{2581}\u{2582}\u{2583}\u{2584}\u{2585}\u{2586}\u{2587}\u{2588}",
        "\u{2502} ok \u{2502}    \u{2502}  \u{2801}\u{2803}\u{2807}\u{2847}\u{28C7}\u{28E7}\u{28F7}\u{28FF}  \u{2190}\u{2191}\u{2192}\u{2193}",
        "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}",
        "",
        "Gallant is the face a Sun workstation boots",
        "into. Twelve pixels wide, twenty-two tall.",
        "The firmware prints two letters and then",
        "waits for you to type:",
    ];

    let bw = 640;
    for (i, &(name, ground, ink, faded, mark)) in bands.iter().enumerate() {
        let x0 = i as i32 * bw;
        frame.fill(Rect::new(x0, 0, bw, 1080), ground);
        let left = x0 + 4 * cw;
        text(&mut frame, &font, left, 2 * ch, name, Pen::new(faded));
        text(&mut frame, &font, left, 4 * ch, "ok", Pen::new(ink).scale(4));
        // The cursor: a full block, as the console draws it.
        frame.fill(Rect::new(left + 3 * cw * 4, 4 * ch, cw * 4, ch * 4), mark);
        text(&mut frame, &font, left, 9 * ch, "Sun Gallant Demi, 12 by 22", Pen::new(faded));
        for (n, line) in lines.iter().enumerate() {
            text(&mut frame, &font, left, (11 + n as i32) * ch, line, Pen::new(ink));
        }
        text(&mut frame, &font, left, 28 * ch, "ok", Pen::new(ink).bold(true));
        frame.fill(Rect::new(left + 3 * cw, 28 * ch, cw, ch), mark);
        text(&mut frame, &font, left, 31 * ch, "The same face at twice", Pen::new(ink).scale(2));
        text(&mut frame, &font, left, 33 * ch, "the size, for a far", Pen::new(ink).scale(2));
        text(&mut frame, &font, left, 35 * ch, "screen or tired eyes.", Pen::new(ink).scale(2));
        text(&mut frame, &font, left, 39 * ch, "A [[linked text]] and a faded aside.", Pen::new(faded));
        text(&mut frame, &font, left + 2 * cw, 39 * ch, "[[linked text]]", Pen::new(mark));
        let c = ink.contrast(ground);
        text(&mut frame, &font, left, 46 * ch, &format!("ink on ground {c:.1} to 1"), Pen::new(faded));
    }

    std::fs::write(&out, encode_png(frame.width(), frame.height(), frame.pixels())).expect("write png");
    println!("{out}: {} glyphs in {}", font.len(), font.name());
}
