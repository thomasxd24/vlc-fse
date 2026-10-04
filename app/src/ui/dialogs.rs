//! Menus, confirmations and text entry — the renderer's `choose()` and `promptText()`, callback-style:
//!
//! ```ignore
//! dialogs::choose(&t("exit.title"), "", vec![choice(&t("exit.stay"), ""), choice(&t("exit.quit"), "quit")], |v| {
//!     if v.as_deref() == Some("quit") { … }
//! });
//! dialogs::prompt(Prompt { title: t("…"), ..Default::default() }, |text| { … });
//! ```
//! `None` means cancelled (Back, or a click on the scrim). Dialogs stack; the newest has focus. When the
//! last one closes, focus goes back to the page (or the top bar, if it had it).

use super::ctx::{cx, Ctx};
use crate::{ChooseSpec, Dialog, Nav, PromptSpec};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

pub use crate::Choice;

type Answer = Box<dyn FnOnce(Option<String>)>;

thread_local! {
    static NEXT: Cell<i32> = const { Cell::new(1) };
    static WAITING: RefCell<HashMap<i32, Answer>> = RefCell::new(HashMap::new());
    static STACK: Rc<VecModel<ChooseSpec>> = Rc::new(VecModel::default());
    static ZONE: RefCell<String> = RefCell::new("page".into());
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let dialog = ui.global::<Dialog>();
    STACK.with(|s| dialog.set_stack(ModelRc::from(s.clone())));
    dialog.on_picked(|id, value| answer(id, if value.is_empty() { None } else { Some(value.to_string()) }));
    dialog.on_prompt_done(|id, ok, value| {
        cx().ui().global::<Dialog>().set_prompt(PromptSpec::default());
        answer(id, if ok { Some(value.to_string()) } else { None });
    });
}

/// A choice with just a label and value; spread it for the rest: `Choice { icon: "play".into(), ..choice(…) }`.
pub fn choice(label: &str, value: &str) -> Choice {
    Choice { label: label.into(), value: value.into(), ..Default::default() }
}

fn next_id() -> i32 {
    NEXT.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    })
}

fn remember_zone() {
    let open = STACK.with(|s| s.row_count()) > 0 || cx().ui().global::<Dialog>().get_prompt().id != 0;
    if !open {
        let zone = cx().ui().global::<Nav>().get_zone().to_string();
        ZONE.with(|z| *z.borrow_mut() = zone);
    }
}

fn restore_focus() {
    let open = STACK.with(|s| s.row_count()) > 0 || cx().ui().global::<Dialog>().get_prompt().id != 0;
    if open {
        return;
    }
    let ui = cx().ui();
    let nav = ui.global::<Nav>();
    if ZONE.with(|z| z.borrow().clone()) == "top" {
        nav.invoke_focus_top();
    } else {
        nav.invoke_focus_page();
    }
}

/// A menu / confirmation. `on_answer` gets the chosen value, or None when cancelled.
pub fn choose(title: &str, text: &str, choices: Vec<Choice>, on_answer: impl FnOnce(Option<String>) + 'static) {
    open(ChooseSpec { title: title.into(), text: text.into(), choices: ModelRc::new(VecModel::from(choices)), ..Default::default() }, on_answer);
}

/// Like [`choose`], with every field of the spec available (wide, initial focus, artwork).
pub fn open(mut spec: ChooseSpec, on_answer: impl FnOnce(Option<String>) + 'static) {
    remember_zone();
    let id = next_id();
    spec.id = id;
    WAITING.with(|w| w.borrow_mut().insert(id, Box::new(on_answer)));
    STACK.with(|s| s.push(spec));
}

#[derive(Default, Clone)]
pub struct Prompt {
    pub title: String,
    pub value: String,
    pub placeholder: String,
    pub ok: String,
    pub secret: bool,
    pub symbols: bool,
}

/// Text entry with the on-screen keyboard. `on_answer` gets the text, or None when cancelled.
pub fn prompt(p: Prompt, on_answer: impl FnOnce(Option<String>) + 'static) {
    remember_zone();
    let id = next_id();
    WAITING.with(|w| w.borrow_mut().insert(id, Box::new(on_answer)));
    cx().ui().global::<Dialog>().set_prompt(PromptSpec {
        id,
        title: p.title.into(),
        value: p.value.into(),
        placeholder: p.placeholder.into(),
        ok: p.ok.into(),
        secret: p.secret,
        symbols: p.symbols,
    });
}

fn answer(id: i32, value: Option<String>) {
    STACK.with(|s| {
        if let Some(i) = s.iter().position(|d| d.id == id) {
            s.remove(i);
        }
    });
    let cb = WAITING.with(|w| w.borrow_mut().remove(&id));
    // The answer first: if it opens another dialog, that one keeps the focus (the page takes focus in a
    // deferred `changed` handler, which would otherwise steal it from the new dialog).
    if let Some(cb) = cb {
        cb(value);
    }
    restore_focus();
}

/// Close the topmost dialog as cancelled. False when none is open.
pub fn cancel_top() -> bool {
    let ui = cx().ui();
    let dialog = ui.global::<Dialog>();
    let p = dialog.get_prompt();
    if p.id != 0 {
        dialog.set_prompt(PromptSpec::default());
        answer(p.id, None);
        return true;
    }
    let top = STACK.with(|s| s.iter().last().map(|d| d.id));
    match top {
        Some(id) => {
            answer(id, None);
            true
        }
        None => false,
    }
}

pub fn is_open() -> bool {
    STACK.with(|s| s.row_count()) > 0 || cx().ui().global::<Dialog>().get_prompt().id != 0
}
