# The website's design language

The site is built to Apple's Human Interface Guidelines (HIG), because the
app is: Rusty Spotlight is drawn on Liquid Glass, so its page should feel
like the same system. Read this before adding a page or a section. It is
not published.

Sources, read November 2026:

- HIG: [Materials](https://developer.apple.com/design/human-interface-guidelines/materials),
  [Color](https://developer.apple.com/design/human-interface-guidelines/color),
  [Typography](https://developer.apple.com/design/human-interface-guidelines/typography),
  [Layout](https://developer.apple.com/design/human-interface-guidelines/layout),
  [Buttons](https://developer.apple.com/design/human-interface-guidelines/buttons),
  [Dark Mode](https://developer.apple.com/design/human-interface-guidelines/dark-mode),
  [Motion](https://developer.apple.com/design/human-interface-guidelines/motion),
  [Writing](https://developer.apple.com/design/human-interface-guidelines/writing)
- [Liquid Glass overview](https://developer.apple.com/documentation/technologyoverviews/liquid-glass)
  and [Adopting Liquid Glass](https://developer.apple.com/documentation/technologyoverviews/adopting-liquid-glass)

We follow the guidelines; we don't copy Apple's pages, artwork or marks,
and the footer says the project isn't Apple's.

## The rules, and where they come from

| Rule on this site | What the HIG says |
| --- | --- |
| **Two layers.** Glass is only the floating toolbar and the launcher panel. Everything you read sits on opaque tiles. | "Liquid Glass forms a distinct functional layer for controls and navigation… Don't use Liquid Glass in the content layer." |
| **Glass is rare.** Two glass things per page, never one on another. | "Use Liquid Glass effects sparingly… avoid layering Liquid Glass elements on top of each other." |
| **The toolbar is regular glass**, the panel is clear glass with a dark dimming layer, because it sits on a colourful stage. | Regular for components with text; clear "only… over visually rich backgrounds", with a dark dimming layer when the content is bright. |
| **Content scrolls under the toolbar and fades at the top edge.** | Use a scroll edge effect rather than a solid bar behind controls. |
| **Colour means "you can press this".** Orange is on the Install button, links and the phrases that can be tried. Headings and labels are never orange. | "Be judicious with your use of color in controls"; "avoid using the same color to mean different things." |
| **System colours, with light, dark and increased-contrast values.** | "Make sure all your app's colors work well in light, dark, and increased contrast contexts." |
| **One prominent button per view**; the second action is a plain link. | "Use a button that has a prominent visual style for the most likely action." |
| **Capsules and concentric corners.** Tiles 28px, things inside them 9 to 12px, controls fully round. | "Rounded shapes that are concentric to their containers." |
| **System fonts in the system's weights**: Regular, Semibold, Bold. Nothing lighter. | "Prefer Regular, Medium, Semibold, or Bold… avoid Ultralight, Thin, and Light." |
| **No theme switch.** The page follows the system. | "Avoid offering an app-specific appearance setting." |
| **Motion is brief and optional.** Presses scale, the list springs, the glass tint fades; all off under Reduce Motion. | "Aim for brevity and precision"; "make motion optional." |
| **Reduce Transparency turns the glass opaque.** | Test with settings that "reduce transparency or motion". |
| **Button and link labels are verbs or plain nouns.** | "Be action oriented… avoid the temptation to be too cute or clever." |

## Colour

System colours as the HIG lists them. Tokens are in `style.css`.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--bg` | `#f2f2f7` | `#000000` | Page (grouped background) |
| `--tile` | `#ffffff` | `#1c1c1e` | Content tiles |
| `--label` | black | white | Text |
| `--label-2` | label at 72% | at 68% | Secondary text |
| `--separator` | 18% | 60% of grey | Hairlines inside tiles, 0.5px |
| `--fill` | grey at 12% | at 24% | Typed text, keys, number badges |
| `--tint` | `#c55300` | `#ffa056` | Orange, accessible variant: links, the button |
| `--orange` | `#ff8d28` | `#ff9230` | System orange: our bar in a chart, the caret. Never text |
| `--green` | `#34c759` | `#30d158` | Switches that are on |

Plain system orange fails contrast as text on white, so text and the filled
button use the HIG's accessible orange.

## Type

`system-ui`, which is San Francisco on Apple devices. Mono is SF Mono, for
anything typed. No fonts are downloaded.

| Style | Size | Weight | Tracking |
| --- | --- | --- | --- |
| Large title (h1) | 44 to 80px | Bold | -0.03em |
| Title (h2) | 32 to 48px | Bold | -0.025em |
| Headline (h3) | 21px | Semibold | -0.018em |
| Lead | 19 to 24px | Regular, secondary | -0.018em |
| Body | 17px / 1.47 | Regular | -0.022em |
| Footnote | 13px | Regular, secondary | -0.006em |

## Layout

- Page 1024px wide, 22px margins (16px on phones).
- The hero is centred. Sections lead from the left: eyebrow, title, lead,
  then tiles 36px below.
- 112px between sections, 16px between tiles, 28px padding inside a tile.
- Lists are grouped lists, as in Settings: one tile, rows divided by inset
  separators.

## Pieces

- **Toolbar**: a glass capsule that floats and stays. Name, two items, the
  Install button.
- **Stage**: a dark tile with large crisp circles in system colours, drawn
  in SVG, so the glass has something real to blur. Shapes stay sharp; we
  never draw blurred blobs.
- **Panel**: the launcher. It works, and its glass takes the colour of the
  selected row, as the app's does.
- **Phrase list**: plugin, what to type, what it means. Orange phrases type
  themselves into the panel.
- **Switches**: the system's shape and green.
- **Figures**: one number per tile; a bar chart when two things compare.
- **Shell block**: always dark, with a capsule Copy button.

## Voice

Short and plain. Say what it does and what to type; give a number where an
adjective would go. Titles end with a full stop. No "blazing", "seamless",
"powerful", no exclamation marks, no emoji.

## Don't

Glass tiles in the content. Glass on glass. Blurred blobs or glows.
Gradients on text or buttons. Orange headings. A second accent colour.
Light font weights. Web fonts. Apple's logo, product images or page copy.
Assets made by capturing someone's screen.
