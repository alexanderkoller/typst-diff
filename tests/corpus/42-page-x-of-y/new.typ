// An extra page is added. "Page X of 2" becomes "Page X of 3" on existing pages.

#set page(
  footer: context {
    let cur = counter(page).get().first()
    let tot = counter(page).final().first()
    align(center)[Page #cur of #tot]
  },
)

= First Section

#lorem(60)

#pagebreak()

= Second Section --- Expanded

#lorem(50)

#pagebreak()

= Third Section

This section was added in the revision.
