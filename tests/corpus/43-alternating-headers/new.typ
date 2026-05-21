// Header text changes from "Draft Version" to "Final Version".
// Body paragraph in Chapter Two also changes.

#set page(
  margin: (top: 2.5cm),
  header: context if calc.rem(counter(page).get().first(), 2) == 1 {
    [_New Report_ #h(1fr) Final Version]
  } else {
    [Final Version #h(1fr) _New Report_]
  },
)

= Chapter One

#lorem(90)

#pagebreak()

= Chapter Two

The updated description of chapter two appears here.

#lorem(60)

#pagebreak()

= Chapter Three

#lorem(70)
