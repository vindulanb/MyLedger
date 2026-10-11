//! The two dashboard charts.
//!
//! Both are drawn here rather than through a JavaScript charting library: the
//! app is Rust compiled to WASM served off a home network, and pulling in a
//! JS bundle for two charts would mean shipping an interop layer, a second
//! package ecosystem, and a theme that doesn't know about this app's colour
//! tokens.
//!
//! The cash-flow chart is HTML and CSS, not SVG, and that is deliberate. An
//! SVG chart scales by scaling its `viewBox`, which scales the text with it —
//! so on a phone the axis labels come out unreadably small, and on a wide
//! screen comically large. Bars that are flex children with percentage
//! heights let the *chart* be fluid while the *labels* stay ordinary text at
//! an ordinary size, which is what "resizes without clipping labels" actually
//! requires.
//!
//! The donut is SVG, because an arc needs a path. It is square and scales
//! cleanly, and its real content — the numbers — lives in the legend beside
//! it, where a screen reader can read it.

use dioxus::prelude::*;
use myledger_core::{CategorySlice, MonthFlow, money};

/// How many colours the donut cycles through before repeating.
const SLICE_COLOURS: usize = 8;

/// Income against expense, one pair of bars per month.
#[component]
pub fn CashFlowChart(points: Vec<MonthFlow>) -> Element {
    if points.is_empty() {
        return rsx! {
            div { class: "chart-empty",
                p { class: "muted", "No income or spending in this period yet." }
            }
        };
    }

    // Both series share one scale, or a month's income would look smaller
    // than a smaller expense beside it.
    let tallest = points
        .iter()
        .map(|p| p.income_cents.max(p.expense_cents))
        .max()
        .unwrap_or(0)
        .max(1);

    let total_income: i64 = points.iter().map(|p| p.income_cents).sum();
    let total_expense: i64 = points.iter().map(|p| p.expense_cents).sum();

    rsx! {
        figure { class: "chart",
            // The whole chart is one image to assistive tech, summarised
            // here; the table below carries the actual figures.
            div {
                class: "chart-plot",
                role: "img",
                "aria-label": "Cash flow over {points.len()} months: {money(total_income)} in, {money(total_expense)} out.",

                for point in points.iter() {
                    div { class: "chart-month", key: "{point.month}",
                        div { class: "chart-bars",
                            div {
                                class: "chart-bar chart-bar-income",
                                style: "height: {percent_of(point.income_cents, tallest)}%",
                                title: "{point.label}: {money(point.income_cents)} in",
                            }
                            div {
                                class: "chart-bar chart-bar-expense",
                                style: "height: {percent_of(point.expense_cents, tallest)}%",
                                title: "{point.label}: {money(point.expense_cents)} out",
                            }
                        }
                        span { class: "chart-label", "{point.label}" }
                    }
                }
            }

            figcaption { class: "chart-key",
                span { class: "key-item",
                    span { class: "key-swatch key-swatch-income" }
                    "In {money(total_income)}"
                }
                span { class: "key-item",
                    span { class: "key-swatch key-swatch-expense" }
                    "Out {money(total_expense)}"
                }
            }
        }
    }
}

/// Spending split by category, with the figures in an accessible legend.
#[component]
pub fn SpendingDonut(slices: Vec<CategorySlice>, total_cents: i64) -> Element {
    if slices.is_empty() {
        return rsx! {
            div { class: "chart-empty",
                p { class: "muted", "No spending in this period yet." }
            }
        };
    }

    // A circle of radius 60 in a 160-unit box. Slices are drawn as arcs by
    // dashing the stroke: each one's dash is its share of the circumference,
    // offset by everything before it.
    const RADIUS: f64 = 60.0;
    let circumference = 2.0 * std::f64::consts::PI * RADIUS;

    let mut offset = 0.0f64;
    let mut arcs = Vec::new();
    for (index, slice) in slices.iter().enumerate() {
        let share = slice.tenths_of_percent as f64 / 1000.0;
        let length = circumference * share;
        arcs.push((index, length, offset, slice.name.clone()));
        offset += length;
    }

    rsx! {
        div { class: "donut-wrap",
            svg {
                class: "donut",
                view_box: "0 0 160 160",
                role: "img",
                "aria-label": "Spending by category, {money(total_cents)} in total across {slices.len()} categories. The figures follow in the legend.",

                // Rotated so the first slice starts at twelve o'clock rather
                // than at three.
                g { transform: "rotate(-90 80 80)",
                    circle {
                        class: "donut-track",
                        cx: "80",
                        cy: "80",
                        r: "{RADIUS}",
                        fill: "none",
                    }
                    for (index, length, start, name) in arcs.iter() {
                        circle {
                            key: "{name}-{index}",
                            class: "donut-arc donut-arc-{index % SLICE_COLOURS}",
                            cx: "80",
                            cy: "80",
                            r: "{RADIUS}",
                            fill: "none",
                            stroke_dasharray: "{length} {circumference}",
                            stroke_dashoffset: "-{start}",
                        }
                    }
                }

                // The middle of a donut is the natural place for the total.
                text {
                    class: "donut-total",
                    x: "80",
                    y: "76",
                    text_anchor: "middle",
                    "aria-hidden": "true",
                    "{money(total_cents)}"
                }
                text {
                    class: "donut-caption",
                    x: "80",
                    y: "94",
                    text_anchor: "middle",
                    "aria-hidden": "true",
                    "spent"
                }
            }

            // A list, not a decoration: this is where the values and
            // percentages live, so they are readable and selectable.
            ul { class: "legend",
                for (index, slice) in slices.iter().enumerate() {
                    li { class: "legend-item", key: "{slice.name}-{index}",
                        span { class: "legend-swatch legend-swatch-{index % SLICE_COLOURS}", "aria-hidden": "true" }
                        span { class: "legend-name", "{slice.name}" }
                        span { class: "legend-value", "{money(slice.cents)}" }
                        span { class: "legend-share", "{slice.percent_label()}" }
                    }
                }
            }
        }
    }
}

/// A value's height as a percentage of the tallest bar.
///
/// A non-zero amount never renders as nothing: a month that spent a little
/// gets a visible sliver rather than a bar indistinguishable from a month
/// that spent nothing at all.
fn percent_of(value: i64, tallest: i64) -> i64 {
    if value <= 0 {
        return 0;
    }
    let share = (value as i128 * 100 / tallest as i128) as i64;
    share.clamp(2, 100)
}
