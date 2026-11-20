use floem::{
    ViewId, peniko::kurbo::Point, prelude::*, reactive::Memo, style::TextOverflow,
};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use umide_workbench_agents::Provider;

use crate::{
    choose_project,
    design::*,
    state::{Workbench, provider_name},
};

fn provider_glyph(provider: Provider) -> Glyph {
    match provider {
        Provider::Codex => Glyph::Terminal,
        Provider::Claude => Glyph::Claude,
    }
}

fn provider_color(provider: Provider) -> Color {
    match provider {
        Provider::Codex => lilac(),
        Provider::Claude => peach(),
    }
}

fn caption(text: &'static str) -> impl IntoView {
    Label::new(text).style(|s| s.font_size(11).ui_bold().color(muted()))
}

fn prompt_shortcut(
    wb: Workbench,
    input: ViewId,
    glyph: Glyph,
    name: &'static str,
    prompt: &'static str,
) -> impl IntoView {
    Button::new(
        Stack::horizontal((icon(glyph, 14., muted), Label::new(name)))
            .style(|s| s.gap(7).items_center()),
    )
    .action(move || {
        wb.draft.set(prompt.into());
        input.request_focus();
    })
    .style(move |s| {
        s.font_size(12)
            .color(ink())
            .background(surface())
            .border(1)
            .border_radius(7)
            .border_color(line())
            .padding_horiz(11)
            .height(32)
            .hover(|s| s.background(raised()))
            .set_disabled(!wb.draft.with(String::is_empty))
    })
}

fn welcome(wb: Workbench, open: bool, input: ViewId) -> impl IntoView {
    let (title, hint) = if open {
        (
            "Start a task",
            "Describe a change to your app, or choose a starting point.",
        )
    } else {
        (
            "Open a project",
            "Choose the folder containing your mobile app to work with an agent.",
        )
    };
    let actions = if open {
        Stack::horizontal((
            prompt_shortcut(wb, input, Glyph::Code, "Review changes",
                "Review the current changes in this mobile project. Identify regressions and suggest concrete fixes."),
            prompt_shortcut(wb, input, Glyph::Device, "Run the app",
                "Inspect this project and explain how to build and run its mobile app on a local device."),
        )).style(|s|s.gap(8)).into_any()
    } else {
        crate::button("Open project…", move || choose_project(wb)).into_any()
    };
    let mark = icon(Glyph::Brand, 25., accent).container().style(|s| {
        s.size(48, 48)
            .items_center()
            .justify_center()
            .background(mint_wash())
            .border_radius(13)
    });
    Stack::vertical((
        mark,
        Stack::vertical((
            Label::new(title).style(|s| s.font_size(30).ui_bold().color(ink())),
            Label::new(hint).style(|s| {
                s.width_full()
                    .font_size(15)
                    .line_height(1.4)
                    .text_overflow(TextOverflow::Wrap)
                    .color(muted())
            }),
        ))
        .style(|s| s.width_full().gap(6)),
        actions,
    ))
    .style(|s| {
        s.width_full()
            .height_full()
            .min_height(0)
            .justify_center()
            .items_start()
            .gap(18)
            .padding_horiz(24)
    })
}

#[derive(Clone, PartialEq)]
enum Block {
    Prose(String),
    Heading(String),
    Code { language: String, body: String },
}

fn reply_blocks(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut events = Parser::new(source).into_offset_iter();
    while let Some((event, range)) = events.next() {
        let Event::Start(tag) = event else {
            let text = source[range].trim();
            if !text.is_empty() {
                blocks.push(Block::Prose(text.to_owned()));
            }
            continue;
        };
        let mut content = String::new();
        let mut destinations = Vec::new();
        let mut depth = 1;
        for (inner, _) in events.by_ref() {
            match inner {
                Event::Text(s)
                | Event::Code(s)
                | Event::Html(s)
                | Event::InlineHtml(s) => content.push_str(&s),
                Event::SoftBreak | Event::HardBreak => content.push('\n'),
                Event::Start(tag) => {
                    depth += 1;
                    if let Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } =
                        tag
                    {
                        destinations.push((content.len(), dest_url));
                    }
                }
                Event::End(tag) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    if matches!(tag, TagEnd::Link | TagEnd::Image)
                        && let Some((offset, url)) = destinations.pop()
                        && content[offset..] != *url
                        && !url.is_empty()
                    {
                        content.push_str(" (");
                        content.push_str(&url);
                        content.push(')');
                    }
                }
                _ => {}
            }
        }
        match tag {
            Tag::CodeBlock(kind) => blocks.push(Block::Code {
                language: match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                },
                body: content.strip_suffix('\n').unwrap_or(&content).to_owned(),
            }),
            Tag::Heading { .. } => blocks.push(Block::Heading(content)),
            Tag::Paragraph => blocks.push(Block::Prose(content)),
            // Preserve ordering, indentation, links and quotations in other Markdown blocks.
            _ => blocks.push(Block::Prose(source[range].trim_end().to_owned())),
        }
    }
    blocks
}

fn response(wb: Workbench, text: Memo<String>) -> impl IntoView {
    let blocks = Memo::new(move |_| text.with(|s| reply_blocks(s)));
    dyn_stack(
        move || blocks.with(|v| 0..v.len()),
        |n| *n,
        move |n| {
            let kind = move || {
                blocks.with(|v| match v.get(n) {
                    Some(Block::Code { .. }) => 2,
                    Some(Block::Heading(_)) => 1,
                    _ => 0,
                })
            };
            dyn_container(kind, move |kind| {
                let body = Label::derived(move || {
                    blocks.with(|v| match v.get(n) {
                        Some(
                            Block::Prose(s)
                            | Block::Heading(s)
                            | Block::Code { body: s, .. },
                        ) => s.clone(),
                        None => String::new(),
                    })
                })
                .label_style(|s| s.selectable(true))
                .style(move |s| {
                    s.width_full()
                        .min_width(0)
                        .text_overflow(TextOverflow::Wrap)
                        .font_size(15)
                        .line_height(1.42)
                        .color(ink())
                        .apply_if(kind == 1, |s| s.font_size(16).ui_bold())
                        .apply_if(kind == 2, |s| {
                            s.font_family(MONO.to_string()).font_size(13)
                        })
                });
                if kind != 2 {
                    return body.into_any();
                }
                let language = Label::derived(move || {
                    blocks.with(|v| match v.get(n) {
                        Some(Block::Code { language, .. })
                            if !language.is_empty() =>
                        {
                            language.clone()
                        }
                        _ => "Code".into(),
                    })
                })
                .style(|s| {
                    s.font_family(MONO.to_string()).font_size(10).color(muted())
                });
                {
                    let copy = Button::new("Copy")
                        .action(move || {
                            let code = blocks.with_untracked(|v| match v.get(n) {
                                Some(Block::Code { body, .. }) => Some(body.clone()),
                                _ => None,
                            });
                            if let Some(code) = code {
                                let result = floem::Clipboard::set_contents(code);
                                wb.status.set(if result.is_ok() {
                                    "Code copied".into()
                                } else {
                                    "Could not copy code to the clipboard".into()
                                });
                            }
                        })
                        .style(|s| {
                            s.font_size(11)
                                .padding_horiz(6)
                                .min_height(21)
                                .border(0)
                                .border_radius(4)
                                .background(panel())
                                .color(muted())
                                .hover(|s| s.background(raised()))
                        });
                    let header = Stack::horizontal((language, copy))
                        .style(|s| s.width_full().justify_between().items_center());
                    Stack::vertical((header, body))
                }
                .style(|s| {
                    s.width_full()
                        .gap(6)
                        .padding(11)
                        .background(panel())
                        .border(1)
                        .border_color(line())
                        .border_radius(8)
                })
                .into_any()
            })
            .style(|s| s.width_full().min_width(0))
        },
    )
    .style(|s| s.width_full().flex_col().gap(8))
}

fn message(wb: Workbench, index: usize, n: usize, agent: Provider) -> impl IntoView {
    let role = move || {
        wb.history.with(|h| {
            h.conversations
                .get(index)
                .and_then(|c| c.messages.get(n))
                .map(|m| m.role.clone())
                .unwrap_or_default()
        })
    };
    let text = Memo::new(move |_| {
        wb.history.with(|h| {
            h.conversations
                .get(index)
                .and_then(|c| c.messages.get(n))
                .map(|m| m.text.clone())
                .unwrap_or_default()
        })
    });
    dyn_container(role, move |role| {
        if role == "assistant" {
            let name = Label::new(provider_name(agent))
                .style(|s| s.font_size(12).ui_bold().color(ink()));
            let icon =
                icon(provider_glyph(agent), 15., move || provider_color(agent));
            let thinking = Label::new("Working…").style(move |s| {
                s.font_size(13)
                    .color(muted())
                    .apply_if(!wb.busy.get() || !text.with(String::is_empty), |s| {
                        s.hide()
                    })
            });
            return Stack::vertical((
                Stack::horizontal((icon, name)).style(|s| s.gap(7).items_center()),
                thinking,
                response(wb, text),
            ))
            .style(|s| s.width_full().gap(7).padding_vert(5))
            .into_any();
        }
        let system = role == "system";
        let name = if system { "Umide" } else { "You" };
        let body = Label::derived(move || text.get())
            .label_style(|s| s.selectable(true))
            .style(|s| {
                s.width_full()
                    .font_size(15)
                    .line_height(1.4)
                    .text_overflow(TextOverflow::Wrap)
            });
        Stack::vertical((
            Label::new(name).style(move |s| {
                s.ui_bold().font_size(11).color(if system {
                    peach()
                } else {
                    muted()
                })
            }),
            body,
        ))
        .style(move |s| {
            s.width_full()
                .min_width(0)
                .padding_horiz(13)
                .padding_vert(10)
                .gap(4)
                .background(surface())
                .border_radius(9)
                .apply_if(system, |s| s.border_left(2).border_color(peach()))
        })
        .into_any()
    })
    .style(|s| s.width_full().min_width(0))
}

fn transcript(wb: Workbench, index: usize) -> impl IntoView {
    let (title, first, agent) = wb.history.with_untracked(|h| {
        let c = &h.conversations[index];
        (
            c.title.clone(),
            c.messages
                .first()
                .filter(|m| m.role == "user")
                .map(|m| m.text.clone())
                .unwrap_or_default(),
            if c.provider == "claude" {
                Provider::Claude
            } else {
                Provider::Codex
            },
        )
    });
    // A short first prompt already serves as the conversation title.
    let promoted = !title.is_empty() && title == first;
    let heading = if title.is_empty() || (!promoted && first.starts_with(&title)) {
        format!("Task with {}", provider_name(agent))
    } else {
        title
    };
    let header = Stack::vertical((
        caption(if promoted {
            "Your request"
        } else {
            "Conversation"
        }),
        Label::new(heading)
            .label_style(|s| s.selectable(true))
            .style(|s| {
                s.width_full()
                    .font_size(23)
                    .ui_bold()
                    .line_height(1.12)
                    .text_overflow(TextOverflow::Wrap)
            }),
    ))
    .style(|s| s.width_full().gap(5).margin_bottom(1));
    let messages = dyn_stack(
        move || {
            wb.history.with(|h| {
                usize::from(promoted)
                    ..h.conversations.get(index).map_or(0, |c| c.messages.len())
            })
        },
        |n| *n,
        move |n| message(wb, index, n, agent),
    )
    .style(|s| s.width_full().flex_col().gap(13));
    let content = Stack::vertical((header, messages)).style(|s| {
        s.width_full()
            .padding_horiz(22)
            .padding_top(19)
            .padding_bottom(20)
            .gap(16)
    });
    // Continue streaming at the bottom while preserving a reader's scroll position.
    let content_height = RwSignal::new(0_f64);
    let following = RwSignal::new(true);
    Scroll::new(content)
        .on_child_size(move |sz| content_height.set(sz.height))
        .on_scroll(move |r| {
            following.set(r.y1 >= content_height.get_untracked() - 28.)
        })
        .scroll_to(move || {
            let h = content_height.get();
            (h > 0. && following.get_untracked()).then_some(Point::new(0., h))
        })
        .style(|s| s.size_full().min_height(0))
}

fn approval(wb: Workbench) -> impl IntoView {
    dyn_container(
        move || {
            wb.current.get().and_then(|index| {
                wb.approvals.with(|items| {
                    items
                        .first()
                        .cloned()
                        .map(|(id, description)| (index, id, description))
                })
            })
        },
        move |request| {
            let Some((index, id, description)) = request else {
                return Empty::new().into_any();
            };
            let allow_id = id.clone();
            let heading = Stack::horizontal((
                icon(Glyph::Shield, 16., peach),
                Label::new("Permission requested")
                    .style(|s| s.ui_bold().font_size(13)),
            ))
            .style(|s| s.items_center().gap(8));
            let detail = Scroll::new(
                Label::new(description)
                    .label_style(|s| s.selectable(true))
                    .style(|s| {
                        s.font_family(MONO.to_string())
                            .width_full()
                            .font_size(12)
                            .text_overflow(TextOverflow::Wrap)
                    }),
            )
            .style(|s| s.max_height(75).width_full());
            Stack::vertical((
                heading,
                detail,
                Stack::horizontal((
                    crate::button("Allow once", move || {
                        wb.decide(index, &allow_id, true)
                    })
                    .style(|s| s.color(accent())),
                    crate::button("Deny", move || wb.decide(index, &id, false)),
                ))
                .style(|s| s.gap(7)),
            ))
            .style(|s| {
                s.width_full()
                    .background(surface())
                    .padding(11)
                    .gap(7)
                    .border_radius(8)
                    .border_left(2)
                    .border_color(peach())
            })
            .into_any()
        },
    )
}

fn workspace_option(wb: Workbench) -> impl IntoView {
    let option = move |name: &'static str, isolated| {
        Button::new(name)
            .action(move || wb.isolated.set(isolated))
            .style(move |s| {
                s.font_size(11)
                    .height(23)
                    .padding_horiz(8)
                    .border(0)
                    .border_radius(4)
                    .color(muted())
                    .background(panel())
                    .hover(|s| s.background(raised()))
                    .apply_if(wb.isolated.get() == isolated, |s| {
                        s.background(raised()).color(ink())
                    })
                    .set_disabled(wb.root.with(Option::is_none))
            })
    };
    let control = dyn_container(
        move || wb.current.get().is_some(),
        move |existing| {
            if existing {
                return Label::derived(move || {
                    if wb.isolated.get() {
                        "Isolated Git copy"
                    } else {
                        "Project folder"
                    }
                })
                .style(|s| s.font_size(11).color(muted()))
                .into_any();
            }
            Stack::horizontal((
                option("Project folder", false),
                option("Isolated Git copy", true),
            ))
            .style(|s| {
                s.padding(2)
                    .gap(2)
                    .background(panel())
                    .border(1)
                    .border_color(line())
                    .border_radius(6)
            })
            .into_any()
        },
    );
    Stack::horizontal((icon(Glyph::Branch, 13., muted), control))
        .style(|s| s.gap(7).items_center())
}

fn provider_switch(wb: Workbench, p: Provider) -> impl IntoView {
    let label = Stack::horizontal((
        icon(provider_glyph(p), 13., move || provider_color(p)),
        Label::new(provider_name(p)),
    ))
    .style(|s| s.gap(5).items_center());
    Button::new(label)
        .action(move || wb.set_provider(p))
        .style(move |s| {
            s.height(27)
                .font_size(11)
                .border_radius(5)
                .padding_horiz(6)
                .border(0)
                .color(muted())
                .background(panel())
                .hover(|s| s.background(raised()))
                .set_disabled(wb.busy.get())
                .apply_if(wb.provider.get() == p, |s| {
                    s.background(raised()).color(ink())
                })
        })
}

pub(super) fn chat(
    wb: Workbench,
    composer_focus: RwSignal<Option<ViewId>>,
) -> impl IntoView {
    let input = text_input(wb.draft)
        .placeholder("Describe a change to your app…")
        .on_enter(move || wb.send())
        .style(move |s| {
            s.width_full()
                .min_width(0)
                .height(39)
                .border(0)
                .background(surface())
                .color(ink())
                .font_size(15)
                .class(floem::views::PlaceholderTextClass, |s| s.color(muted()))
                .padding_horiz(3)
                .focus(|s| s.border(0))
                .set_disabled(wb.root.with(Option::is_none) || wb.busy.get())
        });
    let input_id = input.id();
    composer_focus.set(Some(input_id));
    let conversation = dyn_container(
        move || (wb.current.get(), wb.root.get()),
        move |(index, root)| match index {
            None => welcome(wb, root.is_some(), input_id).into_any(),
            Some(index) => transcript(wb, index).into_any(),
        },
    )
    .style(|s| s.width_full().min_height(0).flex_grow(1_f32).flex_basis(0));
    let agents = Stack::horizontal((
        provider_switch(wb, Provider::Codex),
        provider_switch(wb, Provider::Claude),
    ))
    .style(|s| {
        s.gap(1)
            .background(panel())
            .padding(2)
            .border(1)
            .border_color(line())
            .border_radius(7)
    });
    let screen = Button::new(
        Stack::horizontal((
            dyn_container(
                move || wb.attach.get(),
                move |attached| {
                    let glyph = if attached { Glyph::Check } else { Glyph::Image };
                    icon(
                        glyph,
                        13.,
                        move || if attached { accent() } else { muted() },
                    )
                },
            ),
            Label::derived(move || {
                if wb.attach.get() {
                    "Attached"
                } else {
                    "Screen"
                }
            }),
        ))
        .style(|s| s.items_center().gap(4)),
    )
    .action(move || wb.attach.update(|v| *v = !*v))
    .style(move |s| {
        s.height(29)
            .font_size(11)
            .padding_horiz(6)
            .border(0)
            .border_radius(5)
            .background(surface())
            .color(muted())
            .hover(|s| s.background(raised()))
            .apply_if(wb.attach.get(), |s| {
                s.color(accent()).background(mint_wash())
            })
            .set_disabled(
                wb.busy.get()
                    || (!wb.attach.get() && wb.target.with(Option::is_none)),
            )
            .disabled(|s| s.color(faint()))
    })
    .tooltip(move || {
        Label::new(if wb.attach.get_untracked() {
            "Remove the screen attachment"
        } else if wb.target.with_untracked(Option::is_none) {
            "Select a running device to attach its screen"
        } else {
            "Attach the selected device screen to your next message"
        })
    });
    let send =
        dyn_container(
            move || wb.busy.get(),
            move |busy| {
                let (label, glyph) = if busy {
                    ("Stop", Glyph::Stop)
                } else {
                    ("Send", Glyph::Send)
                };
                let disabled = move || {
                    !busy
                        && (wb.root.with(Option::is_none)
                            || wb.draft.with(|d| d.trim().is_empty()))
                };
                Button::new(
                    Stack::horizontal((
                        icon(glyph, 13., move || {
                            if disabled() { muted() } else { on_accent() }
                        }),
                        Label::new(label),
                    ))
                    .style(|s| s.items_center().gap(5)),
                )
                .action(move || if busy { wb.cancel() } else { wb.send() })
                .style(move |s| {
                    s.height(29)
                        .padding_horiz(8)
                        .font_size(12)
                        .ui_bold()
                        .border(0)
                        .border_radius(6)
                        .color(on_accent())
                        .background(if busy { danger() } else { accent() })
                        .set_disabled(disabled())
                        .disabled(|s| s.background(raised()).color(muted()))
                })
            },
        );
    let toolbar = Stack::horizontal((
        agents,
        screen,
        Empty::new().style(|s| s.flex_grow(1_f32)),
        send,
    ))
    .style(|s| s.width_full().items_center().gap(5));
    let card = Stack::vertical((input, toolbar)).style(|s| {
        s.width_full()
            .min_width(0)
            .padding_horiz(10)
            .padding_top(3)
            .padding_bottom(8)
            .gap(4)
            .border(1)
            .border_color(line())
            .border_radius(10)
            .background(surface())
    });
    let composer = Stack::vertical((approval(wb), workspace_option(wb), card))
        .style(|s| {
            s.width_full()
                .padding_horiz(18)
                .padding_top(4)
                .padding_bottom(13)
                .gap(7)
        });
    Stack::vertical((conversation, composer)).style(|s| s.size_full().min_size(0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_streaming_code_links_and_list_content() {
        let blocks = reply_blocks(
            "# Result\nSee [source](src/lib.rs).\n\n1. Build\n2. Test\n\n```rust\n  let value = 3;",
        );
        assert!(matches!(&blocks[0],Block::Heading(s) if s=="Result"));
        assert!(
            matches!(&blocks[1],Block::Prose(s) if s=="See source (src/lib.rs).")
        );
        assert!(matches!(&blocks[2],Block::Prose(s) if s.contains("2. Test")));
        assert!(
            matches!(&blocks[3],Block::Code{body,language} if language=="rust" && body=="  let value = 3;")
        );
    }
}
