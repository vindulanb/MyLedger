//! The single source of app state in the browser.
//!
//! One rule governs this module: the backend database is canonical. The ledger
//! held here is a *render cache*, nothing more. It is filled by one
//! `GET /api/v1/state` and thrown away and refetched after anything that
//! mutates, so the screen cannot drift from the database. Nothing is persisted
//! to browser storage.

use dioxus::prelude::*;
use myledger_core::StateResponse;

use crate::api;

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
}

impl Store {
    /// Installs the store into context. Call once, at the root.
    pub fn provide() -> Self {
        use_context_provider(|| Self {
            session: Signal::new(Session::Unknown),
            ledger: Signal::new(None),
            load: Signal::new(Load::Idle),
            modal: Signal::new(None),
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

    /// The signed-in user's name, if any.
    pub fn username(&self) -> Option<String> {
        match &*self.session.read() {
            Session::SignedIn { username } => Some(username.clone()),
            _ => None,
        }
    }
}
