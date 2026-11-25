# The website's design language

The rules behind `index.html`, `changelog.html` and `style.css`. Read this
before adding a page or a section. It is not published.

## Why it looks like this

The first site was a dark page with blurred colour blobs, a centred hero, a
gradient pill button and a grid of nine glass cards. That is what every
generated landing page looks like, and people said so.

We looked at how the sites of similar tools are built (November 2026):

| Site | What makes it its own |
| --- | --- |
| Raycast | The product drawn as real interface, a keyboard on the page, one red accent on black. |
| Alfred | Its search box drawn in HTML with real result rows; light page. |
| Sol | Numbered sections (00, 01, 02) and every feature shown by what you type: `42 * 2`. |
| Maccy | Almost no design. "Does one job. Period." The voice carries it. |
| Rectangle, Hammerspoon | Plain, document-like pages that read as written by the author. |
| LaunchBar | Claims as headlines, long light page. |
| Linear, Zed | Hairlines instead of cards, tight type, real interface instead of illustration. |

What the good ones share: a point of view in the words, the product itself
on the page, one accent colour, lines instead of boxes, and numbers.
None of them needs glows.

## The idea: a printed manual for a small tool

The page reads like a well-set manual: paper, ink, one colour, numbered
sections with a margin column. The one loud thing is the product itself: a
launcher you can type in, sitting on a flat field of rust.

## Rules

1. **Show the thing.** The hero is a working imitation of the launcher. A
   feature is explained by what you type and what comes back, never by an
   icon and a blurb.
2. **One colour.** Rust, used for the field, links' underlines, the "New"
   word and the thing that is ours in a chart. Nothing else is coloured.
3. **Lines, not boxes.** Content is separated by hairlines and space. No
   cards, no shadows, no gradients, no blur. The only exceptions are the
   glass panel (it is the product) and the shell block.
4. **Left-aligned.** Nothing is centred except the panel inside its field.
5. **Numbers come with how they were measured.**
6. **No assets we can't draw in CSS.** The only images are the app icon and
   the recorded demo. No stock art, no illustrations, no icon sets.
7. **Nothing from anyone else's server.** System fonts, no trackers, no CDN.

## Colour

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--paper` | `#f3efe6` | `#151310` | Page |
| `--paper-2` | `#e9e4d8` | `#1f1c18` | Typed text, inline code |
| `--ink` | `#1a1815` | `#ece6da` | Text, heavy rules, the main button |
| `--ink-2` | `#56514a` | `#b5ad9f` | Body copy beside a heading |
| `--ink-3` | `#8b857b` | `#7f786d` | Labels, small print |
| `--rule` | ink at 16% | ink at 15% | Hairlines |
| `--rust` | `#b3410f` | `#ea7a40` | The accent |
| `--field` | `#b3410f` | `#a63c0d` | The hero field |

Both themes follow the reader's system setting. The glass panel is the same
in both.

## Type

Three system families, so no font is downloaded.

- **Serif** (`ui-serif`: New York on a Mac, then Iowan Old Style, Palatino,
  Georgia) for headlines and big figures. Regular weight, tight tracking.
  Italic in rust for the one word that lands the headline.
- **Sans** (San Francisco) for running text, 17px.
- **Mono** (SF Mono) for anything typed, and for labels: 12px, capitals,
  tracked 0.08em.

Sizes: h1 42 to 88px, h2 30 to 44px, h3 23px, deck 18 to 21px, small 14px.

## Layout

- Page width 1120px, 24px gutters (16px on phones).
- A section is a 190px margin column (number and name, in mono) beside the
  matter. A 1px ink rule runs across the top of both. On narrow screens the
  margin column sits above.
- 88px between sections, 36px from a section's intro to its content.
- Corner radius: 6px on buttons, 8 to 10px on blocks, 26px on the glass
  panel only.

## Pieces

- **Masthead**: wordmark in serif, three links, hairline under.
- **Field**: flat rust, the shortcut set huge behind the glass in a darker
  rust, the panel on top. A mono note at the bottom says it's an imitation.
- **Phrase row**: plugin name, what to type, what it means. A phrase with a
  rust underline is a button that types itself into the panel.
- **Figures**: big serif numbers in a ruled row, with a plain bar chart
  under them when two things are compared.
- **Steps**: rust serif numerals, ruled rows.
- **Keys**: `<kbd>`, outlined, heavier bottom edge.
- **Changelog entry**: kind in the left column (New in rust; Improved and
  Fixed in grey), serif title, plain text. Dates sit in the margin column.

## Voice

Short, plain, a little dry. Say what it does and what to type. Prefer a
number to an adjective. Admit limits ("this one is an imitation"). No
"blazing", "seamless", "powerful", "supercharge", no exclamation marks, no
emoji.

## Don't

Blurred blobs or glows. Gradients. Glass cards. Pills and badges. Centred
heroes. Grids of feature cards. Icons beside headings. Shadows on content.
A second accent colour. Web fonts. Screenshots as decoration.
