#let card(title, body) = block(stroke: 0.5pt, inset: 6pt)[
  *#title*

  #body
]

#card("Alpha", [Ready for review.])

#card("Beta", [Waiting for approval.])

#card("Gamma", [Scheduled for next week.])
