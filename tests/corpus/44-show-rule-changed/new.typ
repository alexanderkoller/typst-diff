// Show rule: changes to red background box. Text is identical.

#show heading.where(level: 1): it => {
  block(
    fill: rgb("#ffeaea"),
    stroke: (left: 4pt + rgb("#c0392b")),
    inset: (x: 10pt, y: 6pt),
    above: 1.5em,
    below: 0.8em,
    text(fill: rgb("#7a1a1a"), it),
  )
}

= First Heading

Body text under the first heading, unchanged in both versions.

= Second Heading

Body text under the second heading, also unchanged.

= Third Heading

Body text under the third heading, still unchanged.
