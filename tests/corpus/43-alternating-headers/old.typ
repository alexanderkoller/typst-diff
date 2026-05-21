// Alternating odd/even page headers — a common book layout.
// Both the header content AND a body paragraph change in new.typ.
// Expected: headers render on every page with correct odd/even text;
//           the body paragraph diff is annotated correctly.

#set page(
  margin: (top: 2.5cm),
  header: context if calc.rem(counter(page).get().first(), 2) == 1 {
    [_Old Report_ #h(1fr) Draft Version]
  } else {
    [Draft Version #h(1fr) _Old Report_]
  },
)

= Chapter One

#lorem(90)

#pagebreak()

= Chapter Two

The old description of chapter two appears here.

#lorem(60)

#pagebreak()

= Chapter Three

#lorem(70)
