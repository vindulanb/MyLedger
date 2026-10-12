//! Managing the three lookup lists: categories, accounts and tags.
//!
//! Shared rather than written twice because Rules & Tags and Settings both
//! offer tag management, and two screens that delete a tag in two slightly
//! different ways is exactly how a user learns not to trust either of them.
//!
//! One thing governs every delete in here: a lookup row is *soft*-deleted by
//! the backend. It stops being offered, and everything already filed against
//! it keeps resolving to its name. So "delete" never loses history, and the
//! copy says so — the single exception is stripping a tag off past
//! transactions, which does lose something and is therefore opt-in, unticked,
//! and labelled as irreversible.

use dioxus::prelude::*;
use myledger_core::{NamedInput, PreferencesRequest, StateResponse};
use uuid::Uuid;

use crate::store::Store;

/// Which of the three lists a control is working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Categories,
    Accounts,
    Tags,
}

impl ListKind {
    pub fn title(self) -> &'static str {
        match self {
            ListKind::Categories => "Categories",
            ListKind::Accounts => "Accounts",
            ListKind::Tags => "Tags",
        }
    }

    pub fn singular(self) -> &'static str {
        match self {
            ListKind::Categories => "category",
            ListKind::Accounts => "account",
            ListKind::Tags => "tag",
        }
    }

    /// What this list is for, in one sentence.
    pub fn blurb(self) -> &'static str {
        match self {
            ListKind::Categories => {
                "Every transaction is filed under exactly one category. The starter set is a \
                 suggestion, not a fixture — rename, add to or cut it down to whatever matches \
                 how you actually think about your money."
            }
            ListKind::Accounts => {
                "An account is where the money moved: a card, a bank account, cash. Imports match \
                 the name in your statement against these, so the closer these names are to what \
                 your bank calls them, the less you have to correct afterwards."
            }
            ListKind::Tags => {
                "A tag is a free-form label you can put on any number of transactions — handy for \
                 a trip, a project, or anything a category doesn't capture."
            }
        }
    }

    /// The live rows of this list, as `(id, name)`.
    pub fn rows(self, state: &StateResponse) -> Vec<(Uuid, String)> {
        match self {
            ListKind::Categories => {
                state.live_categories().map(|c| (c.id, c.name.clone())).collect()
            }
            ListKind::Accounts => state.live_accounts().map(|a| (a.id, a.name.clone())).collect(),
            ListKind::Tags => state.live_tags().map(|t| (t.id, t.name.clone())).collect(),
        }
    }

    /// Everything in this list that is being kept, as the sync endpoint wants
    /// it: the whole list, minus one, because a group that is present in a
    /// `PUT /preferences` is taken as complete.
    pub fn surviving(self, state: &StateResponse, without: Option<Uuid>) -> Vec<NamedInput> {
        self.rows(state)
            .into_iter()
            .filter(|(id, _)| Some(*id) != without)
            .map(|(id, name)| NamedInput::existing(id, name))
            .collect()
    }

    /// A preferences request carrying only this list.
    pub fn request(self, items: Vec<NamedInput>) -> PreferencesRequest {
        match self {
            ListKind::Categories => {
                PreferencesRequest { categories: Some(items), ..Default::default() }
            }
            ListKind::Accounts => {
                PreferencesRequest { accounts: Some(items), ..Default::default() }
            }
            ListKind::Tags => PreferencesRequest { tags: Some(items), ..Default::default() },
        }
    }

    /// What is still pointing at this row, in words.
    ///
    /// Deleting it would leave those settings naming something the pickers no
    /// longer offer, so the UI refuses and says what to change first. The
    /// backend would accept it — a soft-deleted row still resolves — but
    /// "your budget is for a category that no longer exists" is a confusing
    /// state to walk into later.
    pub fn blockers(self, state: &StateResponse, id: Uuid) -> Vec<String> {
        let mut found = Vec::new();
        match self {
            ListKind::Categories => {
                let budgets =
                    state.budgets.iter().filter(|b| b.category_id == Some(id)).count();
                if budgets > 0 {
                    found.push(plural(budgets, "budget", "budgets"));
                }
                let rules =
                    state.rules.iter().filter(|r| r.then_category_id == Some(id)).count();
                if rules > 0 {
                    found.push(plural(rules, "categorisation rule", "categorisation rules"));
                }
                let items =
                    state.recurring_items.iter().filter(|r| r.category_id == Some(id)).count();
                if items > 0 {
                    found.push(plural(items, "repeating item", "repeating items"));
                }
            }
            ListKind::Accounts => {
                let items =
                    state.recurring_items.iter().filter(|r| r.account_id == Some(id)).count();
                if items > 0 {
                    found.push(plural(items, "repeating item", "repeating items"));
                }
            }
            ListKind::Tags => {
                let rules = state.rules.iter().filter(|r| r.then_tag_id == Some(id)).count();
                if rules > 0 {
                    found.push(plural(rules, "categorisation rule", "categorisation rules"));
                }
            }
        }
        found
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{count} {many}") }
}

/// Joins a list of things into "a, b and c".
pub fn and_list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// How many transactions carry a tag, in words.
pub fn usage_words(uses: i64) -> String {
    match uses {
        0 => "Not used yet".to_string(),
        1 => "On 1 transaction".to_string(),
        many => format!("On {many} transactions"),
    }
}

/// Deleting a tag, and the choice about history.
///
/// The spec asks before stripping a removed tag from past transactions, and
/// the default is to keep them: an old entry that keeps its label keeps its
/// meaning, and the destructive option cannot be undone.
#[component]
pub fn DeleteTagConfirm(
    state: StateResponse,
    tag_id: Uuid,
    tag_name: String,
    uses: i64,
    on_done: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let mut strip = use_signal(|| false);

    let remove = move |_| {
        let wanted = ListKind::Tags.surviving(&state, Some(tag_id));
        let also_strip = strip();
        let done = if also_strip {
            "Tag deleted and taken off your history."
        } else {
            "Tag deleted. Past transactions keep it."
        };
        spawn(async move {
            let request = PreferencesRequest {
                tags: Some(wanted),
                strip_removed_tags: Some(also_strip),
                ..Default::default()
            };
            if store.save_preferences(request, done).await {
                on_done.call(());
            }
        });
    };

    rsx! {
        div { class: "confirmbox", role: "group", "aria-label": "Delete the tag {tag_name}",
            p { class: "confirmbox-title", "Delete “{tag_name}”?" }
            p { class: "muted small",
                if uses == 0 {
                    "Nothing is tagged with it, so nothing else changes."
                } else if uses == 1 {
                    "It stops being offered. One transaction already carries it."
                } else {
                    "It stops being offered. {uses} transactions already carry it."
                }
            }

            if uses > 0 {
                label { class: "checkline",
                    input {
                        r#type: "checkbox",
                        checked: strip(),
                        disabled: busy,
                        onchange: move |event| strip.set(event.checked()),
                    }
                    span {
                        "Also take it off those "
                        if uses == 1 { "transaction" } else { "{uses} transactions" }
                    }
                }
                if strip() {
                    p { class: "formerror", role: "alert",
                        "This cannot be undone. The tag will be gone from your history as well as from the list."
                    }
                } else {
                    p { class: "muted small",
                        "Left unticked, past transactions keep showing the tag — which is why they still make sense when you read them back."
                    }
                }
            }

            div { class: "formactions",
                button {
                    class: "button button-quiet",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_done.call(()),
                    "Keep it"
                }
                button {
                    class: "button button-primary",
                    r#type: "button",
                    disabled: busy,
                    onclick: remove,
                    "Delete tag"
                }
            }
        }
    }
}
