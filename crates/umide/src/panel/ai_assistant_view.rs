use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use floem::{
    View,
    ext_event::{ExtSendTrigger, create_trigger},
    prelude::{SignalGet, SignalUpdate},
    reactive::{ReadSignal, RwSignal},
    views::{Decorators, Label, Scroll, Stack, dyn_stack, text_input},
};
use tokio::sync::oneshot;
use umide_agent::{AgentEvent, Message, ProviderConfig, ProviderKind};

use crate::{
    ai,
    config::{UmideConfig, color::UmideColor},
    panel::position::PanelPosition,
    window_tab::WindowTabData,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MsgRole {
    User,
    Assistant,
}

/// One transcript bubble. Inner fields are signals so streamed deltas update the
/// rendered view in place without rebuilding the whole list.
#[derive(Clone)]
struct ChatMsg {
    id: u64,
    role: MsgRole,
    text: RwSignal<String>,
    tools: RwSignal<Vec<String>>,
}

/// A mutating action awaiting the user's Approve/Reject.
#[derive(Clone)]
struct ApprovalCard {
    id: u64,
    title: String,
    detail: String,
    kind: ai::ApprovalKind,
}

pub fn ai_assistant_panel(
    window_tab_data: Rc<WindowTabData>,
    _position: PanelPosition,
) -> impl View {
    let config = window_tab_data.common.config;
    let scope = window_tab_data.common.scope;
    let workspace = window_tab_data.common.workspace.path.clone();

    let messages: RwSignal<Vec<ChatMsg>> = RwSignal::new(Vec::new());
    let active: RwSignal<Option<ChatMsg>> = RwSignal::new(None);
    let input = RwSignal::new(String::new());
    let streaming = RwSignal::new(false);
    let next_id = RwSignal::new(0u64);
    let key_input = RwSignal::new(String::new());
    // Default to the first provider that already has a key (keychain or env).
    let initial_kind = ProviderKind::all()
        .into_iter()
        .find(|&k| ProviderConfig::resolve(k, ai::load_api_key(k)).is_ok())
        .unwrap_or(ProviderKind::Anthropic);
    let provider_kind = RwSignal::new(initial_kind);
    let has_key = RwSignal::new(
        ProviderConfig::resolve(initial_kind, ai::load_api_key(initial_kind))
            .is_ok(),
    );
    let status = RwSignal::new(if has_key.get_untracked() {
        format!(
            "{} ready — read-only, so your editor and emulators stay untouched.",
            initial_kind.label()
        )
    } else {
        format!(
            "Add your {} API key below to enable it.",
            initial_kind.label()
        )
    });

    // Lossless cross-thread bridge: the worker pushes AgentEvents into `queue`
    // and pulses `trigger`; a UI-thread effect tracks the trigger and drains the
    // whole queue (so no streamed delta is ever coalesced away).
    let queue: ai::EventQueue = Arc::new(Mutex::new(VecDeque::new()));
    let approvals: ai::ApprovalQueue = Arc::new(Mutex::new(VecDeque::new()));
    let history: Arc<Mutex<Vec<Message>>> = Arc::new(Mutex::new(Vec::new()));
    let trigger = create_trigger();
    // Set by the Stop button to abort an in-flight turn; reset on each send.
    let cancel = Arc::new(AtomicBool::new(false));

    // UI-thread-only: the cards shown to the user, and the oneshot senders that
    // resolve each approval when a button is clicked.
    let pending: RwSignal<Vec<ApprovalCard>> = RwSignal::new(Vec::new());
    let senders: Rc<RefCell<HashMap<u64, oneshot::Sender<ai::ApprovalOutcome>>>> =
        Rc::new(RefCell::new(HashMap::new()));

    {
        let queue = queue.clone();
        let approvals = approvals.clone();
        let senders = senders.clone();
        scope.create_effect(move |_| {
            trigger.track();
            let events: Vec<AgentEvent> =
                { queue.lock().unwrap().drain(..).collect() };
            for ev in events {
                apply_event(ev, active, status, streaming);
            }
            let reqs: Vec<ai::ApprovalRequest> =
                { approvals.lock().unwrap().drain(..).collect() };
            for req in reqs {
                senders.borrow_mut().insert(req.id, req.respond);
                pending.update(|v| {
                    v.push(ApprovalCard {
                        id: req.id,
                        title: req.title,
                        detail: req.detail,
                        kind: req.kind,
                    })
                });
            }
        });
    }

    let transcript = Scroll::new(
        dyn_stack(
            move || messages.get(),
            |m| m.id,
            move |m| message_view(m, config),
        )
        .style(|s| s.flex_col().width_full().padding(8.0)),
    )
    .style(|s| s.flex_grow(1.0).width_full());

    let status_line = Label::derived(move || status.get()).style(move |s| {
        s.width_full()
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .font_size(11.0)
            .color(config.get().color(UmideColor::PANEL_FOREGROUND))
    });

    let send = send_handler(
        trigger,
        workspace,
        history,
        queue,
        approvals,
        cancel.clone(),
        provider_kind,
        input,
        messages,
        active,
        next_id,
        streaming,
        status,
    );

    let input_box = text_input(input).style(move |s| {
        s.flex_grow(1.0)
            .padding(6.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.get().color(UmideColor::LAPCE_BORDER))
    });
    let cancel_btn = cancel.clone();
    let send_button = Stack::new((Label::derived(move || {
        if streaming.get() {
            "Stop".to_string()
        } else {
            "Send".to_string()
        }
    }),))
    .on_click_stop(move |_| {
        if streaming.get_untracked() {
            cancel_btn.store(true, Ordering::Relaxed);
        } else {
            send();
        }
    })
    .style(move |s| {
        s.padding_horiz(12.0)
            .padding_vert(6.0)
            .items_center()
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.get().color(UmideColor::LAPCE_BORDER))
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| {
                s.background(floem::peniko::Color::from_rgba8(255, 255, 255, 20))
            })
    });
    let input_row = Stack::new((input_box, send_button))
        .style(|s| s.width_full().items_center().padding(6.0));

    let approvals_view = {
        let senders = senders.clone();
        let wtd = window_tab_data.clone();
        dyn_stack(
            move || pending.get(),
            |c| c.id,
            move |c| approval_card(c, wtd.clone(), config, senders.clone(), pending),
        )
        .style(|s| s.flex_col().width_full().padding(6.0))
    };

    // Key entry — only visible until a key is stored (keychain or env).
    let key_box = text_input(key_input).style(move |s| {
        s.flex_grow(1.0)
            .padding(6.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.get().color(UmideColor::LAPCE_BORDER))
    });
    let save_key = pill_button("Save key", config, move || {
        let key = key_input.get_untracked().trim().to_string();
        if key.is_empty() {
            return;
        }
        let kind = provider_kind.get_untracked();
        match ai::store_api_key(kind, &key) {
            Ok(()) => {
                has_key.set(true);
                key_input.set(String::new());
                status.set(format!(
                    "{} key saved to your keychain — ask about your codebase.",
                    kind.label()
                ));
            }
            Err(e) => status.set(format!("could not save key: {e}")),
        }
    });
    let key_row = Stack::new((key_box, save_key)).style(move |s| {
        let s = s.width_full().items_center().padding(6.0);
        if has_key.get() { s.hide() } else { s }
    });

    // Provider selector — Claude / OpenAI / DeepSeek / Gemini (BYO key each).
    let provider_row = Stack::new((
        provider_button(
            ProviderKind::Anthropic,
            provider_kind,
            has_key,
            status,
            config,
        ),
        provider_button(
            ProviderKind::OpenAi,
            provider_kind,
            has_key,
            status,
            config,
        ),
        provider_button(
            ProviderKind::DeepSeek,
            provider_kind,
            has_key,
            status,
            config,
        ),
        provider_button(
            ProviderKind::Gemini,
            provider_kind,
            has_key,
            status,
            config,
        ),
    ))
    .style(|s| s.width_full().items_center().padding(6.0));

    Stack::new((
        provider_row,
        transcript,
        approvals_view,
        status_line,
        key_row,
        input_row,
    ))
    .style(|s| s.flex_col().size_pct(100.0, 100.0))
}

/// A provider tab in the selector row; highlights when it's the active provider.
fn provider_button(
    kind: ProviderKind,
    provider_kind: RwSignal<ProviderKind>,
    has_key: RwSignal<bool>,
    status: RwSignal<String>,
    config: ReadSignal<Arc<UmideConfig>>,
) -> impl View {
    Stack::new((Label::new(kind.label()),))
        .on_click_stop(move |_| {
            provider_kind.set(kind);
            let ok = ProviderConfig::resolve(kind, ai::load_api_key(kind)).is_ok();
            has_key.set(ok);
            status.set(if ok {
                format!("{} ready — read-only.", kind.label())
            } else {
                format!("Add your {} API key below to enable it.", kind.label())
            });
        })
        .style(move |s| {
            let active = provider_kind.get() == kind;
            let s = s
                .padding_horiz(10.0)
                .padding_vert(4.0)
                .border_radius(6.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .color(config.get().color(UmideColor::PANEL_FOREGROUND));
            if active {
                s.border(1.0)
                    .border_color(config.get().color(UmideColor::LAPCE_BORDER))
                    .background(floem::peniko::Color::from_rgba8(255, 255, 255, 28))
            } else {
                s.border(1.0)
                    .border_color(floem::peniko::Color::from_rgba8(0, 0, 0, 0))
            }
        })
}

/// Render one approval card with Approve/Reject buttons. Approving an edit
/// applies it through the editor here on the UI thread (undo-preserving).
fn approval_card(
    card: ApprovalCard,
    window_tab_data: Rc<WindowTabData>,
    config: ReadSignal<Arc<UmideConfig>>,
    senders: Rc<RefCell<HashMap<u64, oneshot::Sender<ai::ApprovalOutcome>>>>,
    pending: RwSignal<Vec<ApprovalCard>>,
) -> impl View {
    let id = card.id;
    let kind = card.kind.clone();
    let resolve = move |outcome: ai::ApprovalOutcome| {
        if let Some(tx) = senders.borrow_mut().remove(&id) {
            let _ = tx.send(outcome);
        }
        pending.update(|v| v.retain(|c| c.id != id));
    };
    let approve = {
        let resolve = resolve.clone();
        let kind = kind.clone();
        let wtd = window_tab_data.clone();
        pill_button("Approve", config, move || {
            let outcome = match &kind {
                ai::ApprovalKind::Command => ai::ApprovalOutcome::CommandApproved,
                ai::ApprovalKind::Edit {
                    path,
                    old_str,
                    new_str,
                } => match ai::apply_edit_in_editor(&wtd, path, old_str, new_str) {
                    Ok(()) => ai::ApprovalOutcome::EditApplied,
                    Err(e) => ai::ApprovalOutcome::EditFailed(e),
                },
            };
            resolve(outcome);
        })
    };
    let reject = pill_button("Reject", config, move || {
        resolve(ai::ApprovalOutcome::Rejected)
    });

    Stack::new((
        Label::new(card.title).style(move |s| {
            s.font_size(12.0)
                .color(config.get().color(UmideColor::PANEL_FOREGROUND))
        }),
        Label::new(card.detail).style(move |s| {
            s.width_full()
                .font_size(11.0)
                .color(config.get().color(UmideColor::PANEL_FOREGROUND))
        }),
        Stack::new((approve, reject)).style(|s| s.padding_vert(4.0)),
    ))
    .style(move |s| {
        s.flex_col()
            .width_full()
            .padding(8.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.get().color(UmideColor::LAPCE_BORDER))
    })
}

fn pill_button(
    label: &'static str,
    config: ReadSignal<Arc<UmideConfig>>,
    on_click: impl Fn() + 'static,
) -> impl View {
    Stack::new((Label::new(label),))
        .on_click_stop(move |_| on_click())
        .style(move |s| {
            s.padding_horiz(12.0)
                .padding_vert(4.0)
                .margin_right(6.0)
                .border(1.0)
                .border_radius(6.0)
                .items_center()
                .border_color(config.get().color(UmideColor::LAPCE_BORDER))
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| {
                    s.background(floem::peniko::Color::from_rgba8(255, 255, 255, 20))
                })
        })
}

/// Apply one streamed event to the transcript (runs on the UI thread).
fn apply_event(
    ev: AgentEvent,
    active: RwSignal<Option<ChatMsg>>,
    status: RwSignal<String>,
    streaming: RwSignal<bool>,
) {
    match ev {
        AgentEvent::TextDelta(t) => {
            if let Some(a) = active.get_untracked() {
                a.text.update(|s| s.push_str(&t));
            }
        }
        AgentEvent::ThinkingDelta(_) => {}
        AgentEvent::ToolCallStarted { name, .. } => {
            if let Some(a) = active.get_untracked() {
                a.tools.update(|v| v.push(format!("⚙ {name}…")));
            }
        }
        AgentEvent::ToolCallInput { .. } => {}
        AgentEvent::ToolResult {
            name, ok, summary, ..
        } => {
            if let Some(a) = active.get_untracked() {
                let mark = if ok { "✓" } else { "✗" };
                a.tools
                    .update(|v| v.push(format!("  ↳ {mark} {name}: {summary}")));
            }
        }
        AgentEvent::TurnComplete { usage } => {
            status.set(format!(
                "tokens — in {} · out {} · cache-read {}",
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens
            ));
        }
        AgentEvent::Done => streaming.set(false),
        AgentEvent::Error(e) => {
            status.set(format!("error: {e}"));
            streaming.set(false);
        }
    }
}

fn message_view(m: ChatMsg, config: ReadSignal<Arc<UmideConfig>>) -> impl View {
    let role = match m.role {
        MsgRole::User => "You",
        MsgRole::Assistant => "Assistant",
    };
    let text = m.text;
    let tools = m.tools;
    Stack::new((
        Label::new(role).style(move |s| {
            s.font_size(11.0)
                .color(config.get().color(UmideColor::PANEL_FOREGROUND))
        }),
        Label::derived(move || text.get()).style(move |s| {
            s.width_full()
                .color(config.get().color(UmideColor::PANEL_FOREGROUND))
        }),
        Label::derived(move || tools.get().join("\n")).style(move |s| {
            s.width_full()
                .font_size(11.0)
                .color(config.get().color(UmideColor::PANEL_FOREGROUND))
        }),
    ))
    .style(|s| s.flex_col().width_full().padding_vert(6.0))
}

#[allow(clippy::too_many_arguments)]
fn send_handler(
    trigger: ExtSendTrigger,
    workspace: Option<PathBuf>,
    history: Arc<Mutex<Vec<Message>>>,
    queue: ai::EventQueue,
    approvals: ai::ApprovalQueue,
    cancel: Arc<AtomicBool>,
    provider_kind: RwSignal<ProviderKind>,
    input: RwSignal<String>,
    messages: RwSignal<Vec<ChatMsg>>,
    active: RwSignal<Option<ChatMsg>>,
    next_id: RwSignal<u64>,
    streaming: RwSignal<bool>,
    status: RwSignal<String>,
) -> impl Fn() + 'static {
    move || {
        if streaming.get_untracked() {
            return;
        }
        let text = input.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }

        let kind = provider_kind.get_untracked();
        let provider = match ProviderConfig::resolve(kind, ai::load_api_key(kind)) {
            Ok(p) => p,
            Err(_) => {
                status.set(format!(
                    "Add your {} API key below to enable it.",
                    kind.label()
                ));
                return;
            }
        };

        let id = next_id.get_untracked();
        next_id.set(id + 2);
        let user = ChatMsg {
            id,
            role: MsgRole::User,
            text: RwSignal::new(text.clone()),
            tools: RwSignal::new(Vec::new()),
        };
        let assistant = ChatMsg {
            id: id + 1,
            role: MsgRole::Assistant,
            text: RwSignal::new(String::new()),
            tools: RwSignal::new(Vec::new()),
        };
        messages.update(|m| {
            m.push(user);
            m.push(assistant.clone());
        });
        active.set(Some(assistant));
        input.set(String::new());
        streaming.set(true);
        status.set("thinking…".into());

        cancel.store(false, Ordering::Relaxed);
        ai::spawn_turn(
            workspace.clone(),
            provider,
            history.clone(),
            text,
            queue.clone(),
            approvals.clone(),
            trigger,
            cancel.clone(),
        );
    }
}
