// Footer: "Page X of Y" using counter(page).final() — requires context.
// In new.typ an extra page is added, changing Y from 2 to 3.
// Expected: footer renders correctly on all pages of the diff PDF.

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

= Second Section

#lorem(50)
