# ok: the locked design

This is the contract the interface is built from. Where code and this file
disagree, one of them is a bug.

## Thesis

**ok is a Sun console set up as a typewriter: a sheet of Gallant lying on a
desk, the line you are writing held still while the paper climbs past it, and
a machine that says one word, ok, only when what you typed is on the disk.**

Swap in iA Writer, Obsidian, Vim or Word and the sentence is false, so it is a
thesis.

## What governs it

The Sun workstation console at its `ok` prompt. From it: the face (Gallant,
12 by 22), a steady block cursor, inverse video as the one native highlight,
one line where you talk to the machine, the word `ok` printed when the machine
has finished what you gave it, and settings as a plain table of names and
values.

Borrowed for what each really was, never for atmosphere:

| From | What |
| --- | --- |
| The typewriter | The platen: the paper moves, the typing point does not. Underscore for emphasis, double strike for bold. A sheet on a desk. |
| WordPerfect 5.1 | A page with nothing on it but one line at the foot, and every question asked on that line. |
| WordStar | The flag column beside the text; help that shows only if you hesitate. |
| XyWrite, EVE | One key that takes you from the page to a line of plain words and back. |
| DEC VT100 | The double-size line, which is how a heading is made without a second face. |
| troff, DEC manuals | Change bars beside lines that are new. |
| The newsroom | TK for "to come". |
| ed | Answering a write with the number of bytes written. |

No boot text, no scanlines, no glow, no fake log, no Sun logo, no sound.

## The one screen

Everything is drawn on a cell grid. With Gallant at scale 1 a 1920 by 1080
borderless window is 160 columns by 49 rows (1 pixel of ground above and
below). There are two surfaces: the **sheet** (the page and the machine's two
lines) and the **desk** (everything around it). In a look where they are the
same colour the screen is one flat ground.

Columns at 160:

| Columns | What |
| --- | --- |
| 0 to 1 | desk margin |
| 2 to 36 | contents: the writing folder as a contents page |
| 37 to 40 | desk |
| 41 to 118 | the sheet, full height |
| 42 to 44 | hanging marks (`#`, `>`), right-aligned to 44 |
| 45 | change bars |
| 47 to 112 | the text: a 66-column measure |
| 113 | where the cursor rests after a full row |
| 119 | desk |
| 120 | notch: where the cursor is in the map |
| 121 to 131 | the map |
| 132 | map flags |
| 134 to 157 | labels (headings) and, at the bottom, links |
| 158 to 159 | desk margin |

Rows at 49:

| Rows | What |
| --- | --- |
| 0 to 44 | the text. Row 28 is the platen. |
| 45 | always empty |
| 46 | the say row: the one place the machine speaks in sentences |
| 47 | the machine line: `ok` at columns 47 to 48, typing from 50, facts right-aligned to 112 |
| 48 | empty |

Any other size: columns and rows are the client size over the cell size, the
leftover pixels are split evenly, the measure is the setting or columns minus
14 if that is smaller, the sheet is the measure plus 12, and what is left is
split between the sides. The contents are drawn when the left side has 24
columns or more; the map when the right side has 15 or more; labels when it
has 27 or more. The platen is always row `round(0.62 * text rows)`. When a
list that should answer the line is not on screen, its rows rise from the say
row over the foot of the sheet, eight at most.

Nothing is ruled or boxed. Regions are separated by empty cells and by the two
surfaces.

### The sheet

Markup is never hidden and never moves: every cell of text is a character
that is in the file, so arrow keys never lie. Marks are drawn at half
strength and the words they govern take the attribute.

- `# heading`: double size (each glyph at twice the scale, two rows tall, 33
  to the measure). The `#` hangs in the gutter at single size, half strength.
  When the cursor is on it the cursor is two cells wide and two rows tall.
- `## heading`: single size, double struck. `###` and deeper: single size,
  plain. Their marks hang in the gutter too.
- `**strong**`: double strike (the glyph drawn again one pixel to the right).
- `*emphasis*`, `_emphasis_`: the letters with an underscore struck in the
  same cell. There is no slanted face and none is faked.
- Lists: marker at half strength, wrapped rows hang under the item's first
  letter. `- [ ]` and `- [x]` are shown as typed.
- `> quote`: the `>` hangs in the gutter at half strength; the words are set
  in from both edges of the measure by 4.
- `[[link]]`: brackets at half strength, name in ink. A link whose text does
  not exist yet has its name at half strength too. When the cursor is inside
  a link the whole link is inverse.
- `TK` standing alone in capitals: inverse.
- Code spans, tables and rules are shown as typed.

Inverse video means exactly: the cursor, a selection, find hits, the link the
cursor is in, TK, and the row Enter would act on in a list. Double strike
outside the text is used for nothing.

Change bars: a 2-pixel stroke in column 45 beside every row written or
changed today. Today begins at 04:00.

The cursor is a solid block and never blinks. When the line has the cursor,
the page shows a hollow box where you will return.

### The contents (left)

The folder as a contents page. Folder names flush left, texts indented two
cells per level, leader dots, word count right-aligned to column 36. Order is
alphabetical and never changes with use. The open text is in ink; everything
else at half strength; leader dots faint. Shown: `.md`, `.markdown`, `.txt`.
Names starting with a dot are not listed. A folder taller than the pane
scrolls to keep the open text in view.

### The map (right)

The whole text in miniature with the same line breaks as the page: each
letter a 2 by 2 pixel square, each row 4 pixels, spaces left open, so words
read as dashes and paragraphs as blocks. A double-size heading is twice as
big here too. It never shrinks to fit: a longer text makes a longer map,
which is the point. Past 253 rows it changes once to 1 by 1 pixels and 2 per
row; past that it scrolls to keep the notch in view.

On a desk the map is a strip of the sheet's own paper, 4 pixels of margin
round the marks, and the strip grows as the text does.

Rows written today are ink; older rows half strength; rows past a word limit
faint. The notch is a 6 by 2 pixel bar in column 120 level with the cursor's
row. Flags in column 132: a tick beside each row holding a TK and, while a
find is open, beside each row holding a hit.

Labels: each heading's words at half strength on the cell row its line falls
in (the section the cursor is in, in ink). A text with no headings lists its
paragraphs by their opening words, faint. If two labels want the same row the
later moves down.

Links, bottom-aligned to row 45 in the same columns:

```
See also
  Grandfather interview
Linked from
  Essay plan
```

Up to six rows each, then `and 4 more`. A link to a text that does not exist
yet ends in a space and a question mark.

## Keys

Always typing. No modes. A letter is always a letter, from the first frame.
There are exactly two places the cursor can be: the page and the line. Esc
moves between them.

Sizes, one table:

|  | Left, Right | Up, Down | Backspace, Delete |
| --- | --- | --- | --- |
| plain | letter | row | letter |
| Ctrl | word | paragraph | word |
| Alt | sentence | carries the paragraph up or down | sentence |

Shift with any movement selects.

| Keys | Does |
| --- | --- |
| Enter | Ends the paragraph and writes the blank line between paragraphs. In a list, the next item; on an empty item, ends the list. On a blank line or in fenced code, one line break. |
| Shift Enter | A line break inside the paragraph. |
| Backspace at the start of a paragraph | Undoes what Enter did: removes the blank line and the break together. |
| Tab, Shift Tab | In a list, nests the item or brings it back by two spaces. After `[[`, takes the offered name and closes the brackets. Otherwise nothing: no tab characters. |
| Home, End | Ends of the row. With Ctrl, of the text. |
| Page Up, Page Down | One screen less a row. |
| Ctrl Z, Ctrl Y, Ctrl Shift Z | Undo, redo. |
| Ctrl X, Ctrl C, Ctrl V, Ctrl A | Clipboard, plain text. Select all. |
| Ctrl S | Keeps a version and answers with the truth: `Kept 14:02. 2,481 bytes on disk.` |
| Ctrl Enter | Follows the link the cursor is in, starting the text if it does not exist. A web address opens in the browser. |
| Ctrl T | To the next TK, wrapping. |
| Ctrl F | Esc and `/` in one. F3 and Shift F3 step through hits. |
| Ctrl = and Ctrl - | Scale 1, 2, 3. |
| F11 | Between borderless full screen and a window. |
| F1 | The keys, on the page. |
| Esc | To the line, and back. |
| Ctrl Q, Alt F4 | Leaves. Nothing is asked because nothing can be lost. |

Hold Ctrl or Alt for 500 ms without pressing anything else and the say row
prints that key's row of the table until you let go. That is how the keys are
found.

Never bound: anything on Ctrl with Alt (that is AltGr on other keyboards).

The mouse may work: a click places the cursor or opens a row in a list, the
wheel scrolls, a click on the map goes there. The pointer hides at the first
keystroke and returns when moved. Nothing needs it.

## The line

Esc puts the solid cursor after `ok` on the machine line. Nothing pops up:
the line drives lists that are already on the screen, and the say row above
it always states, in one sentence starting with Enter, what Enter will do
now. Arrows try, Enter keeps, Esc puts back.

The first character or word decides:

| You type | It does | Answered by |
| --- | --- | --- |
| nothing, then Enter | Goes back to where you last were. | contents |
| a title | Finds a text: words in any order, any case. Up and Down choose. The page shows the text under the bar once it has rested 250 ms. If nothing matches: `Enter starts a new text called Rebuttal notes.` A backslash makes or uses a folder. | contents |
| `#` and letters | Goes to a heading (or, with none, a paragraph). The page follows as you type. | labels |
| `/` and a phrase | Finds it in this text. Hits go inverse; the nearest after the cursor comes to the platen. Case is ignored unless you type a capital. | the page |
| `/old/new` | Changes it. Enter changes this one and moves on; `/old/new/g` changes all, and Ctrl Z takes them all back. | the page |
| `rename` and a title | Renames the text and mends every link to it. | contents |
| `set` | Every option, as a table of names and values in the contents pane. Lists are walked with Up and Down and the whole screen changes as the bar lands; numbers are typed or nudged; colours are typed as hex. Enter keeps, Esc puts back, `default` restores. The line stays at `ok set` for the next one. | contents |
| `earlier` | This text as it was: versions newest first. Enter brings one back; the present one is kept too. | contents |
| `discard` | Moves this text out of the folder into `.discarded`. Nothing is ever erased. | |
| `copy` | Puts the whole text on the clipboard with the marks removed, each paragraph one line. | |
| `keys` | The keys, on the page. | |
| `notices` | The licences of the faces and code inside the .exe, on the page. | |
| `quit` | Leaves. | |

Left empty for 1200 ms, the line shows what it understands, faint, and removes
it at the first character:
`a title   # heading   / phrase   rename   set   earlier   discard   copy   keys`

A mistake gets `?` and one sentence on the say row, as ed would:
`? No option called mesure. The nearest is measure.` Nothing beeps, nothing
blocks, and the next key clears it.

On the page, typing `[[` wakes the contents even in a run, filtered to the
letters typed since; the best match is offered after the cursor at half
strength; Tab takes it. Typing `]]` yourself keeps exactly what you typed.

## Flow: what the screen does while you write

Three strengths of ink on each surface: ink, half (65 percent of the way from
ground to ink) and faint (35 percent). Half stays readable; faint is for what
you are not meant to read just then.

| State | Trigger | The screen |
| --- | --- | --- |
| Rest | Launch, and after Settle | Text ink, marks half. Contents half with the open text in ink. Labels half. Facts on the machine line half. |
| Writing | The 8th character typed with no gap over 2 s | Contents, labels, links and the facts go to faint over 900 ms. The pointer hides. |
| Run | 20 words typed since then with no gap of 4 s | Every paragraph but yours goes to half over 1500 ms. Contents, labels, links and facts go to ground. |
| Breath | 4 s without a key, in Writing or Run | All paragraphs back to ink over 400 ms. Nothing else returns. |
| Settle | 10 s without a key | Everything back over 400 ms, numbers current. If 100 words or more went by: `143 words without stopping.` on the say row for 6 s. |
| Taking the wheel | The cursor leaves its paragraph, Esc, any chord, the mouse | Straight to Rest in 120 ms. |

Never dimmed in any state: the paragraph you are in, the cursor, the map's
marks, the notch, the change bars, `ok`.

Enter during a run leaves the paragraph just finished in ink until you are 8
characters into the next. Backspace, Delete and movement inside the paragraph
do not change the state.

Speed is the absence of delay: a key press mutates the text and paints in the
same message, with no easing and no gliding caret. When a row wraps or Enter
is pressed at the platen, the paper moves up and the typing row stays where it
was; with `scroll smooth` that takes four frames, and a key that arrives
mid-roll finishes it at once.

`ok` is erased by the first keystroke that changes the text and printed again
when the whole text has been written and flushed and nothing has changed
since. A burst, a pause, ok.

A word limit (`set limit 650`) puts a rule of equals signs and `650 words`
on the sheet where the limit falls; words past it are half strength on the
page and faint in the map, and the facts read `688 of 650 words`. No colour.

Everything here can be turned off: `set focus off`, `set scroll jump`,
`set echo off`, `set platen off`. If Windows has animations turned off every
fade is a single step.

## What the machine says

Sentence case, no exclamation marks, no congratulation, never a question that
must be answered before typing. Each fits 66 cells.

```
Start typing. Each word is saved as you finish it.
Esc goes to the line. Hold Ctrl or Alt to see what they do.
Last here Tuesday at 23:41. That sitting added 388 words.
143 words without stopping.
Kept 14:02. 2,481 bytes on disk.
Not saved. This folder cannot be written. Still trying.
Saved again. Nothing was lost.
Brought back 41 words typed after the last save.
This text changed on disk. The newer one is shown; yours is kept.
This file is not UTF-8, so it is open for reading only.
Enter opens College\The orchard.
Enter starts a new text called Rebuttal notes.
Enter goes back to where you were, in Opening statement.
Enter goes to Where it is weak.
3 of 12. Up and Down step through them. Enter stays here.
Enter keeps measure at 66. Esc puts it back to 72. The default is 66.
Enter renames this text and mends 2 links to it.
Changed 12. Ctrl Z takes them all back.
Moved to .discarded. Nothing was erased.
Copied 412 words with the marks removed.
2 still to come. Ctrl T goes to the first.
Nothing selected. Shift with an arrow selects.
```

The facts on the machine line, right-aligned, at rest:
`The orchard    412 of 650 words    163 today`. The name is the file's name
without its ending. `today` counts words typed today across the folder, never
falls when you cut, and is absent when it would be 0. With a selection the
count reads `38 words selected`.

A failed write is the only alarming thing the app can show: the machine line
goes inverse and stays so until the write succeeds.

## Looks

A look is seven values. Every colour is named for the thing it is taken from.

| Token | Is |
| --- | --- |
| `paper` | the sheet |
| `ink` | text on the sheet |
| `desk` | everything round the sheet |
| `desk-ink` | text on the desk |
| `mark` | the cursor, the notch, change bars. Defaults to ink, so most looks are one-bit. |
| `half`, `faint` | a percentage of the way from ground to ink, or a colour |

Built in:

| Look | The object | paper, ink, desk, desk-ink | Status |
| --- | --- | --- | --- |
| `manila` (default) | A cream typing sheet on a stained walnut desk, struck through a black fabric ribbon; the words on the desk are the colour of a manila tag lying on it | EEE3C8, 221F1B, 35251A, E6D9BA | Desk: derived from measured black walnut (Moslemi 1967, USDA Forest Service NC-17), hue and chroma kept, lightness lowered. Ink: derived, the warm lean of black on uncoated paper (ISO 12647-2 type 5), darkened until half strength reads at 4.5 to 1. Paper and manila: estimates by eye. A flatbed scan of his own sheet, envelope and a ribbon strike beside a grey card replaces all four. |
| `sun` | The Sun console: a white framebuffer, black Gallant | FFFFFF, 000000 | Sourced: colour map entries 0 and ff of Sun's cgthree driver. |
| `acme` | Plan 9's acme: pale yellow frames, black type | FFFFEA, 000000 | Sourced: plan9port acme.c and draw.h. |
| `register`, `register night` | The paper and ink of jaspermeller.com | F4F5F1, 1E1F1C and 20211F, E9EAE4 | Sourced: his stylesheet. |
| `amber` | An amber data tube of 1982 to 1985 | 191310, FFB000 | P3 amber at full level, bright enough that half strength reads and can still be told from ink. |
| `vt100` | A DEC VT100: P4 white on unlit glass | 151819, 9BB7CB | Phosphor and chromaticity sourced; levels estimated. |
| `5151` | The IBM Monochrome Display: P39 green | 101913, 00CB75 | Chromaticity sourced; this is the tube's blue-leaning emerald, not `00FF00`. |
| `wordperfect` | WordPerfect 5.1 on VGA: grey on blue, half strength in cyan | 0000AA, AAAAAA, half 00AAAA | Sourced: VGA palette entries 1, 7, 3. |

Body text is at least 4.5 to 1 on its ground in every built-in look, and so
is half strength wherever it carries words.

## Faces

Gallant 12 by 22 is built in and is the default. Vaughan Pratt drew it in
April 1982 for the Sun PROM monitor; Jef Poskanzer carried it into 4.4BSD;
Jens Schweikhardt extended it to 5,320 glyphs in 2025 and 2026. The caption
is "Gallant", which is what Sun's own source calls it.

Also built in: Spleen 12 by 24 (OpenBSD's console face) and Terminus 11 by
22 (kept as its untouched BDF, as its licence requires). Any `.hex`, `.bdf`,
`.psf`, `.ttf` or `.otf` dropped into the `faces` folder appears by its file
name, and so does any fixed-pitch face installed in Windows (Courier New is
the typewriter). Outline faces are rasterized through GDI into the grid; the
cell comes from the face.

A character the face lacks is drawn from Gallant when the cell allows, and
otherwise as a small box holding its hex digits. It is never invisible and
never a question mark: U+FFFD is drawn only for a real U+FFFD in the file.

## Saving, as felt

There is no save command because there is nothing to save, and no dialog
anywhere. Every edit goes to a journal at once; the file itself is rewritten
whole and swapped in atomically a moment after you pause. `ok` means the file
on disk is what is on screen. Leaving asks nothing. Coming back puts the same
text on the same row with the cursor in the same cell. After a crash the
journal is replayed and the say row says how many words came back. `earlier`
walks back through kept versions.

The writing folder stays clean: nothing in it but his texts and, only after a
discard, `.discarded`. The journal, versions, settings and state live under
`%LOCALAPPDATA%\ok` and `%APPDATA%\ok`.

A file that is not valid UTF-8 opens for reading only and is never rewritten.
A file's line endings and byte-order mark are kept as they were.

## Settings

`set` on the line, or the settings text itself (`%APPDATA%\ok\settings.txt`,
one `name value` per line, the same words the line takes, read again whenever
it changes).

`look`, `face`, `scale` (1 to 3), `paper`, `ink`, `desk`, `desk-ink`, `mark`,
`half`, `faint`, `measure` (40 to 100, default 66), `platen` (on, off),
`scroll` (smooth, jump), `cursor` (block, underline), `focus` (on, off),
`echo` (on, off), `enter` (paragraph, line), `limit` (words, for this text
only), `window` (full, framed), `folder`.

Setting a colour by hand makes `look` read `custom`; typing a new name after
`set look` keeps the present colours under that name.

## First run and return

First run: the window opens on the sheet with no banner. `Documents\ok` has
been made with one empty text. The cursor sits on the platen. The say row
reads `Start typing. Each word is saved as you finish it.` The first line he
finishes names the text. After his first pause the say row reads
`Esc goes to the line. Hold Ctrl or Alt to see what they do.` once. He is
never asked for a file name, a folder or a preference before he can write.

Every return: the same text, the same row on the platen, the cursor in the
same cell, and for as long as he does not type:
`Last here Tuesday at 23:41. That sitting added 388 words.`

## Left out on purpose

Modes, hjkl, counts, registers, macros. A blinking cursor. Word-rate meters,
streaks, badges, timers, a clock. Hidden markup and live preview. A gliding
caret, per-letter effects, sound. Spelling marks while typing, autocorrect,
smart quotes. Tabs, splits, a second window, any dialog. An accent colour.
Rich text, images, tables, export, printing. Erasing a file. Sync, accounts,
anything that needs a network.
