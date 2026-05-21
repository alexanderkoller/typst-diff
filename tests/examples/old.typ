#set page(
  margin: (x: 2.5cm, y: 2.5cm),
  footer: context [
    #set text(size: 9pt, fill: luma(140))
    Quarterly Performance Analysis
    #h(1fr)
    Page #counter(page).display() of #counter(page).final().first()
  ],
)
#set heading(numbering: "1.")
#set par(justify: true)
#show heading.where(level: 1): it => {
  v(0.6em)
  text(fill: rgb("#1a3a5c"), it)
  v(0.2em)
  line(length: 100%, stroke: 0.5pt + rgb("#1a3a5c").lighten(60%))
  v(0.1em)
}

#align(center)[
  #text(size: 20pt, weight: "bold")[Quarterly Performance Analysis]
  #v(0.3em)
  #text(size: 11pt, fill: luma(80))[Engineering Division — Q3 Report]
  #v(0.2em)
  #text(size: 10pt, fill: luma(120))[Prepared by the Technical Review Committee]
]

#v(1.2em)

= Executive Summary

The engineering division delivered *strong results* in the third quarter,
exceeding the projected output by *12 percent*.
Key drivers included the early completion of the _cache redesign_ and
improved collaboration between the backend and infrastructure teams.
Resource utilisation averaged 84% across all active projects,
with *no critical incidents* reported during the review period.

= Key Metrics

#table(
  columns: (1.6fr, 1fr, 1fr, 1fr),
  inset: 7pt,
  stroke: 0.5pt + luma(200),
  fill: (_, row) => if row == 0 { luma(230) } else { white },
  [*Metric*],            [*Target*],      [*Actual*],       [*Variance*],
  [Throughput (req/s)],  [4 200],         [4 650],          [+10.7%],
  [Error rate],          [< 0.5%],        [0.31%],          [−0.19 pp],
  [P95 latency (ms)],    [120],           [108],            [−10.0%],
  [Deploy frequency],    [weekly],        [twice weekly],   [+100%],
)

= Observations

The following issues were noted during the review period:

- The logging pipeline introduced in July consumed *more memory than expected*,
  peaking at *6.2 GB* under sustained load.
- Cross-region replication lag exceeded the 500 ms threshold on *three occasions*,
  each correlated with scheduled batch jobs.
- Documentation coverage for new API endpoints stands at *68%*,
  below the team target of 80%.

= Forecast

Assuming the current growth trajectory holds, projected throughput $T$ for Q4
satisfies $T = T_0 (1 + r)^n$ where $T_0 = 4650$, $r = 0.08$, and $n = 1$,
yielding an estimate of approximately *5 022 req/s*.
The team will prioritise memory optimisation and documentation completeness in Q4.
