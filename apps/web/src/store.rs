//! The single source of app state in the browser.
//!
//! One rule governs this module: the backend database is canonical. The ledger
//! held here is a *render cache*, nothing more. It is filled by one
//! `GET /api/v1/state` and thrown away and refetched after anything that
//! mutates, so the screen cannot drift from the database. Nothing is persisted
//! to browser storage.

use dioxus::prelude::*;
use myledger_core::{Period, PreferencesRequest, StateResponse};

use crate::api;

/// Today's date in the browser's own time zone, as `YYYY-MM-DD`.
///
/// Local, not UTC: a transaction entered at 9pm on the 31st belongs to the
/// 31st as far as the person entering it is concerned, and every period
/// boundary is reckoned the same way.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Whether we are signed in, as far as the browser can tell.
///
/// `Unknown` is the startup state: the session is in an `HttpOnly` cookie that
/// script cannot read, so the only way to find out is to ask the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Session {
    Unknown,
    SignedIn { username: String },
    SignedOut,
}

/// How the ledger fetch is going.
#[derive(Debug, Clone, PartialEq)]
pub enum Load {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

/// A short-lived message shown over the app, for the outcome of an action.
///
/// Errors stay until dismissed; successes fade. Both are transient UI, not
/// state — nothing is ever read back out of here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub is_error: bool,
}

/// A dialog owned by the app shell rather than by a page, because the top bar
/// can open it from anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    /// The global "Add entry" action. Built in step 3.
    AddEntry,
    /// The global "Import" action. Built in step 4.
    Import,
}

/// Shared app state. `Copy`, because every field is a `Signal`, so it can be
/// handed to any component through context without cloning the ledger.
#[derive(Clone, Copy)]
pub struct Store {
    pub session: Signal<Session>,
    pub ledger: Signal<Option<StateResponse>>,
    pub load: Signal<Load>,
    pub modal: Signal<Option<Modal>>,
    /// The latest action's outcome, shown as a toast.
    pub notice: Signal<Option<Notice>>,
    /// A period the user has just chosen but the server hasn't confirmed.
    ///
    /// Shown immediately so the selector feels instant, and dropped either
    /// way once the server answers: on success the refreshed ledger carries
    /// the new value, and on failure dropping it *is* the revert.
    pub pending_period: Signal<Option<Period>>,
}

impl Store {
    /// Installs the store into context. Call once, at the root.
    pub fn provide() -> Self {
        use_context_provider(|| Self {
            session: Signal::new(Session::Unknown),
            ledger: Signal::new(None),
            load: Signal::new(Load::Idle),
            modal: Signal::new(None),
            notice: Signal::new(None),
            pending_period: Signal::new(None),
        })
    }

    /// Reads the store back out of context.
    pub fn use_store() -> Self {
        use_context::<Self>()
    }

    /// Asks the server who we are, and loads the ledger if we're signed in.
    ///
    /// This is the only way to learn whether the cookie is still good.
    /// Resolves the session, and nothing else.
    ///
    /// Loading the ledger is deliberately not done here. A spawned task
    /// belongs to the scope that spawned it and is cancelled when that scope
    /// unmounts, so work started during sign-in dies the moment the login
    /// screen is replaced. The app shell owns the ledger fetch instead,
    /// because it mounts exactly when there is a session and outlives every
    /// screen inside it.
    pub fn restore_session(self) {
        let mut store = self;
        spawn(async move {
            match api::me().await {
                Ok(me) => store.session.set(Session::SignedIn { username: me.username }),
                Err(_) => store.session.set(Session::SignedOut),
            }
        });
    }

    /// Refetches the whole ledger.
    ///
    /// Call this from a scope that will still be mounted when the request
    /// comes back — the app shell, or a handler on a page inside it. Calling
    /// it from a component that is about to be replaced loses the response,
    /// because the task goes with the scope.
    pub fn refresh(self) {
        spawn(async move { self.load_ledger().await });
    }

    /// Fetches the ledger. Awaitable, so it composes inside another task.
    ///
    /// Refetching all of it rather than patching the copy in memory is
    /// deliberate: a partial update is how a UI starts disagreeing with its
    /// database.
    pub async fn load_ledger(mut self) {
        {
            self.load.set(Load::Loading);
            match api::state().await {
                Ok(state) => {
                    self.ledger.set(Some(state));
                    self.load.set(Load::Ready);
                }
                // The session lapsed underneath us — go back to the login
                // screen rather than showing an empty ledger as though the
                // data were gone.
                Err(failure) if failure.is_unauthorized() => {
                    self.ledger.set(None);
                    self.load.set(Load::Idle);
                    self.session.set(Session::SignedOut);
                }
                Err(failure) => self.load.set(Load::Failed(failure.message)),
            }
        }
    }

    /// Records a successful sign-in.
    ///
    /// Only flips the session; the shell that this reveals is what fetches the
    /// ledger, on mount.
    pub fn signed_in(mut self, username: String) {
        self.session.set(Session::SignedIn { username });
    }

    /// Ends the session, server-side and locally.
    pub fn sign_out(mut self) {
        spawn(async move {
            // Whether or not the call succeeds, stop showing the ledger: the
            // user asked to be signed out.
            let _ = api::logout().await;
            self.ledger.set(None);
            self.load.set(Load::Idle);
            self.modal.set(None);
            self.session.set(Session::SignedOut);
        });
    }

    /// Opens one of the global dialogs.
    pub fn open(mut self, modal: Modal) {
        self.modal.set(Some(modal));
    }

    /// Closes whatever dialog is open.
    pub fn close_modal(mut self) {
        self.modal.set(None);
    }

    /// The period the dashboard and transactions list are showing.
    ///
    /// An unconfirmed choice wins over the stored one so the selector
    /// responds at once; everything else reads through here, which is why
    /// changing the period on one screen changes it on the other.
    pub fn period(&self) -> Period {
        if let Some(pending) = *self.pending_period.read() {
            return pending;
        }
        self.ledger
            .read()
            .as_ref()
            .map(|state| state.settings.selected_period)
            .unwrap_or(Period::AllTime)
    }

    /// Changes the period, saving it to the server.
    ///
    /// Optimistic: the new value shows immediately, and a failure puts the
    /// old one back and says why, rather than leaving the screen claiming a
    /// period the database doesn't have.
    pub fn set_period(self, period: Period) {
        let mut store = self;
        if store.period() == period {
            return;
        }
        store.pending_period.set(Some(period));

        spawn(async move {
            let request =
                PreferencesRequest { selected_period: Some(period), ..Default::default() };
            match api::put_preferences(&request).await {
                Ok(_) => {
                    // Refetch before clearing, so the selector never flickers
                    // back through the old value on its way to the new one.
                    store.load_ledger().await;
                    store.pending_period.set(None);
                }
                Err(failure) => {
                    store.pending_period.set(None);
                    store.fail(failure);
                }
            }
        });
    }

    /// Reports a failed call.
    ///
    /// A lapsed session is not an error to show — it means the cookie expired
    /// underneath us, so the right response is the login screen.
    pub fn fail(mut self, failure: api::ApiFailure) {
        if failure.is_unauthorized() {
            self.ledger.set(None);
            self.load.set(Load::Idle);
            self.modal.set(None);
            self.session.set(Session::SignedOut);
            return;
        }
        self.notice.set(Some(Notice { text: failure.message, is_error: true }));
    }

    /// Dismisses the current toast.
    pub fn clear_notice(mut self) {
        self.notice.set(None);
    }

    /// The signed-in user's name, if any.
    pub fn username(&self) -> Option<String> {
        match &*self.session.read() {
            Session::SignedIn { username } => Some(username.clone()),
            _ => None,
        }
    }
}
