// Running header: shows the most recent level-1 heading via context query.
// In new.typ: header formatting changes (emph → bold) and chapter 2 title changes.
// Expected: the diff PDF renders correct running headers on every page.

#set page(
  margin: (top: 3cm),
  header: context {
    let h = query(heading.where(level: 1))
    let pg = here().page()
    let before = h.filter(it => it.location().page() <= pg)
    if before.len() > 0 {
      align(right, emph(before.last().body))
    }
  },
)

= First Chapter

#lorem(80)

#pagebreak()

= Second Chapter

#lorem(80)
