// Running header: style changes from emph to bold, chapter 2 title changes.
// The header on page 2 must read "Second Chapter — Revised" in bold.

#set page(
  margin: (top: 3cm),
  header: context {
    let h = query(heading.where(level: 1))
    let pg = here().page()
    let before = h.filter(it => it.location().page() <= pg)
    if before.len() > 0 {
      align(right, text(weight: "bold", before.last().body))
    }
  },
)

= First Chapter

#lorem(80)

#pagebreak()

= Second Chapter --- Revised

#lorem(80)
