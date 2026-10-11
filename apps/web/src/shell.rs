//! The app shell: sidebar, top bar, and the mobile bottom nav.
//!
//! One list of destinations drives all three, so a page cannot appear in the
//! sidebar and go missing from the bottom nav. The prompt's requirement that
//! every tab be reachable on a narrow screen is therefore structural rather
//! than something to remember.

use dioxus::prelude::*;

use crate::Route;
use crate::components::{AddEntryForm, Toast};
use crate::store::{Load, Modal, Store};

/// A destination, with the icon used in both navs.
struct Destination {
    route: Route,
    label: &'static str,
    /// SVG path data, stroked. Hand-written rather than pulled from an icon
    /// package to keep the WASM bundle to what it needs.
    icon: &'static str,
}

fn destinations() -> Vec<Destination> {
    vec![
        Destination {
            route: Route::Dashboard {},
            label: "Dashboard",
            icon: "M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z",
        },
        Destination {
            route: Route::Transactions {},
            label: "Transactions",
            icon: "M4 7h16M4 12h16M4 17h10",
        },
        Destination {
            route: Route::Recurring {},
            label: "Recurring",
            icon: "M4 12a8 8 0 0 1 13.6-5.7M20 12a8 8 0 0 1-13.6 5.7M17 3v3.6h-3.6M7 21v-3.6h3.6",
        },
        Destination {
            route: Route::Subscriptions {},
            label: "Subscriptions",
            icon: "M3 7h18v10H3zM3 11h18M7 15h4",
        },
        Destination {
            route: Route::Budgets {},
            label: "Budgets",
            icon: "M5 20V11M12 20V4M19 20v-6",
        },
        Destination {
            route: Route::Goals {},
            label: "Goals",
            icon: "M6 21V4h12l-3 4 3 4H6",
        },
        Destination {
            route: Route::Documents {},
            label: "Documents",
            icon: "M6 3h7l5 5v13H6zM13 3v5h5",
        },
        Destination {
            route: Route::RulesAndTags {},
            label: "Rules & Tags",
            icon: "M3 7h11l6 5-6 5H3zM7 11h.01",
        },
        Destination {
            route: Route::Settings {},
            label: "Settings",
            icon: "M4 8h10M18 8h2M4 16h6M12 16h8M14 5v6M10 13v6",
        },
    ]
}

/// The title shown in the top bar for the current route.
fn title_for(route: &Route) -> &'static str {
    destinations()
        .iter()
        .find(|destination| destination.route == *route)
        .map(|destination| destination.label)
        .unwrap_or("Not found")
}

#[component]
pub fn Shell() -> Element {
    let store = Store::use_store();

    // The shell owns the ledger fetch. It mounts once per signed-in session
    // and outlives every page inside it, so the response cannot be lost to a
    // scope being torn down — which is what happens if the login screen starts
    // the fetch and is then replaced by this.
    use_hook(move || store.refresh());

    // And for the same reason it owns saving the period. The selector is on
    // every page, so a request it started itself would be cancelled by the
    // user navigating — which left the selection stuck as pending and the
    // whole control disabled. Here the task belongs to the shell and survives
    // the navigation that caused it.
    use_effect(move || {
        let Some(period) = *store.pending_period.read() else {
            return;
        };
        spawn(async move { store.apply_period(period).await });
    });

    rsx! {
        div { class: "shell",
            Sidebar {}
            div { class: "shell-body",
                TopBar {}
                main { class: "shell-content",
                    if let Load::Failed(message) = store.load.read().clone() {
                        div { class: "banner banner-error", role: "alert",
                            span { "{message}" }
                            button {
                                class: "button button-quiet",
                                onclick: move |_| store.refresh(),
                                "Try again"
                            }
                        }
                    }
                    Outlet::<Route> {}
                }
            }
            BottomNav {}
            ModalHost {}
            Toast {}
        }
    }
}

#[component]
fn NavIcon(path: &'static str) -> Element {
    rsx! {
        svg {
            class: "nav-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.7",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: path }
        }
    }
}

#[component]
fn Sidebar() -> Element {
    rsx! {
        nav { class: "sidebar", "aria-label": "Main",
            div { class: "sidebar-mark", "MyLedger" }
            ul { class: "sidebar-list",
                for destination in destinations() {
                    li { key: "{destination.label}",
                        Link {
                            to: destination.route.clone(),
                            class: "sidebar-link",
                            active_class: "is-active",
                            NavIcon { path: destination.icon }
                            span { "{destination.label}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TopBar() -> Element {
    let store = Store::use_store();
    let route = use_route::<Route>();
    let busy = matches!(*store.load.read(), Load::Loading);

    rsx! {
        header { class: "topbar",
            div { class: "topbar-heading",
                h1 { class: "topbar-title", "{title_for(&route)}" }
                if busy {
                    span { class: "topbar-sync", "Refreshing…" }
                }
            }

            div { class: "topbar-actions",
                button {
                    class: "button button-quiet",
                    onclick: move |_| store.open(Modal::Import),
                    "Import"
                }
                button {
                    class: "button button-primary",
                    onclick: move |_| store.open(Modal::AddEntry),
                    "Add entry"
                }
                if let Some(username) = store.username() {
                    div { class: "topbar-user",
                        span { class: "topbar-username", "{username}" }
                        button {
                            class: "button button-quiet",
                            onclick: move |_| store.sign_out(),
                            "Sign out"
                        }
                    }
                }
            }
        }
    }
}

/// The narrow-screen nav.
///
/// Scrolls horizontally rather than collapsing into a "more" menu, so every
/// destination stays one tap away instead of two.
#[component]
fn BottomNav() -> Element {
    rsx! {
        nav { class: "bottomnav", "aria-label": "Main",
            div { class: "bottomnav-track",
                for destination in destinations() {
                    Link {
                        key: "{destination.label}",
                        to: destination.route.clone(),
                        class: "bottomnav-link",
                        active_class: "is-active",
                        NavIcon { path: destination.icon }
                        span { class: "bottomnav-label", "{destination.label}" }
                    }
                }
            }
        }
    }
}

/// Hosts the dialogs the top bar can open from any page.
///
/// The contents of each arrive with the step that builds them; the plumbing —
/// which component owns the dialog, how it closes — is settled here so those
/// steps only have to fill in a body.
#[component]
fn ModalHost() -> Element {
    let store = Store::use_store();
    let Some(modal) = *store.modal.read() else {
        return rsx! {};
    };

    let title = match modal {
        Modal::AddEntry => "Add entry",
        Modal::Import => "Import",
    };

    rsx! {
        div {
            class: "modal-backdrop",
            onclick: move |_| store.close_modal(),

            div {
                class: "modal",
                role: "dialog",
                "aria-modal": "true",
                "aria-label": "{title}",
                // Stop a click inside the panel from reaching the backdrop and
                // closing the dialog.
                onclick: move |event| event.stop_propagation(),

                header { class: "modal-head",
                    h2 { class: "modal-title", "{title}" }
                    button {
                        class: "button button-quiet",
                        "aria-label": "Close",
                        onclick: move |_| store.close_modal(),
                        "Close"
                    }
                }
                div { class: "modal-body",
                    match modal {
                        Modal::AddEntry => rsx! {
                            AddEntryForm { on_done: move |_| store.close_modal() }
                        },
                        Modal::Import => rsx! {
                            p { class: "muted", "CSV statement import arrives with the import step." }
                        },
                    }
                }
            }
        }
    }
}
