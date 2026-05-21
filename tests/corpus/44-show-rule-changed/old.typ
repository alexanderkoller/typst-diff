// Show rule for level-1 headings: blue text.
// In new.typ: show rule changes to red background box.
// Text is identical in both versions.
// Expected: all heading and body blocks Equal — no textual change.

#show heading.where(level: 1): it => {
  set text(fill: rgb("#1a4a7a"))
  block(above: 1.5em, below: 0.8em, it)
}

= First Heading

Body text under the first heading, unchanged in both versions.

= Second Heading

Body text under the second heading, also unchanged.

= Third Heading

Body text under the third heading, still unchanged.
