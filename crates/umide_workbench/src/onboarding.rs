//! First-run setup for the local coding agents.
use crate::design::{button, canvas};
use floem::{prelude::*, style::TextOverflow};
use umide_workbench_agents::Provider;

use crate::{
    design::*,
    i18n::{self, tr},
    provider_setup::{ProviderSetup, Stage},
    state::provider_name,
};

fn status_text(stage: &Stage) -> String {
    match stage {
        Stage::Checking => tr("Checking this computer…").into(),
        Stage::Missing => tr("Command-line tool not installed").into(),
        Stage::Installing => tr("Installing on this computer…").into(),
        Stage::SignedOut(version) if version.is_empty() => {
            tr("Installed · sign-in needed").into()
        }
        Stage::SignedOut(version) => {
            i18n::fmt("Sign-in needed · {version}", &[("version", version)])
        }
        Stage::SigningIn => tr("Complete sign-in in your browser…").into(),
        Stage::Ready(version) if version.is_empty() => tr("Connected").into(),
        Stage::Ready(version) => {
            i18n::fmt("Connected · {version}", &[("version", version)])
        }
        Stage::Error(detail) => i18n::message(detail),
    }
}

fn provider_card(setup: ProviderSetup, provider: Provider) -> impl IntoView {
    let (glyph, tint): (_, fn() -> Color) = match provider {
        Provider::Codex => (Glyph::Codex, lilac),
        Provider::Claude => (Glyph::Claude, peach),
    };
    let mark = icon(glyph, 23., tint).container().style(|s| {
        s.size(44, 44)
            .flex_shrink(0_f32)
            .border_radius(10)
            .background(surface())
            .items_center()
            .justify_center()
    });
    let name = Label::new(provider_name(provider))
        .style(|s| s.font_size(18).ui_bold().color(ink()));
    let status = Label::derived(move || status_text(&setup.stage(provider))).style(
        move |s| {
            let color = match setup.stage(provider) {
                Stage::Ready(_) => accent(),
                Stage::Error(_) => danger(),
                _ => muted(),
            };
            s.width_full()
                .font_size(13)
                .color(color)
                .text_overflow(TextOverflow::Wrap)
        },
    );
    let info = Stack::vertical((name, status))
        .style(|s| s.flex_grow(1_f32).min_width(0).gap(4));
    let action = dyn_container(
        move || setup.stage(provider),
        move |stage| match stage {
            Stage::Checking => Empty::new().into_any(),
            Stage::Missing => {
                button("Install", move || setup.install(provider)).into_any()
            }
            Stage::Installing | Stage::SigningIn => {
                button("Cancel", move || setup.cancel(provider)).into_any()
            }
            Stage::SignedOut(_) => {
                button("Sign in", move || setup.sign_in(provider)).into_any()
            }
            Stage::Ready(_) | Stage::Error(_) => {
                button("Check again", move || setup.refresh(provider)).into_any()
            }
        },
    )
    .style(|s| s.flex_shrink(0_f32));
    let guide = Button::new(
        Stack::horizontal((
            i18n::label("Setup guide"),
            icon(Glyph::Open, 11., faint),
        ))
        .style(|s| s.items_center().gap(5)),
    )
    .action(move || setup.guide(provider))
    .style(|s| {
        s.font_size(12)
            .color(muted())
            .padding_horiz(4)
            .margin_left(53)
            .min_height(24)
            .background(crate::design::panel())
            .border(0)
            .border_radius(4)
            .hover(|s| s.color(ink()).background(surface()))
            .focus_visible(|s| s.outline(2).outline_color(accent()))
    })
    .tooltip(move || {
        i18n::fmt(
            "Open the official {provider} setup guide",
            &[("provider", provider_name(provider))],
        )
    });
    let top = Stack::horizontal((mark, info, action))
        .style(|s| s.width_full().items_center().gap(13));
    Stack::vertical((top, guide)).style(|s| {
        s.width_full()
            .gap(6)
            .padding(16)
            .items_start()
            .border(1)
            .border_color(subtle_line())
            .border_radius(10)
            .background(crate::design::panel())
    })
}

pub fn panel(
    setup: ProviderSetup,
    finish: impl Fn() + Clone + 'static,
) -> impl IntoView {
    let logo = icon(Glyph::Brand, 25., accent).container().style(|s| {
        s.size(47, 47)
            .border_radius(12)
            .items_center()
            .justify_center()
            .background(mint_wash())
    });
    let intro = Stack::vertical((
        i18n::label("Set up your coding agents").style(|s| s.font_size(30).ui_bold().color(ink())),
        i18n::label("Install Codex or Claude Code on this computer. Sign in with your provider to use it in Umide.")
            .style(|s| s.width_full().font_size(15).line_height(1.4).color(muted()).text_overflow(TextOverflow::Wrap)),
    ))
    .style(|s| s.width_full().gap(7));
    let providers = Stack::vertical((
        provider_card(setup, Provider::Codex),
        provider_card(setup, Provider::Claude),
    ))
    .style(|s| s.width_full().gap(10));
    let both = button("Install both", move || {
        setup.install(Provider::Codex);
        setup.install(Provider::Claude);
    })
    .style(move |s| {
        s.apply_if(
            setup.stage(Provider::Codex) != Stage::Missing
                || setup.stage(Provider::Claude) != Stage::Missing,
            |s| s.hide(),
        )
    });
    let proceed = Button::new(i18n::label("Continue to workspace"))
        .action(finish)
        .style(|s| {
            s.height(34)
                .padding_horiz(13)
                .items_center()
                .justify_center()
                .font_size(13)
                .ui_bold()
                .color(on_accent())
                .background(accent())
                .border(0)
                .border_radius(7)
                .hover(|s| s.border(1).border_color(ink()))
                .focus_visible(|s| s.outline(2).outline_color(ink()))
        });
    let footer = Stack::horizontal((
        both,
        Empty::new().style(|s| s.flex_grow(1_f32)),
        proceed,
    ))
    .style(|s| s.width_full().gap(8).items_center());
    let top = Stack::horizontal((
        logo,
        Empty::new().style(|s| s.flex_grow(1_f32)),
        crate::i18n::picker(false).style(|s| s.width(190)),
    ))
    .style(|s| s.width_full().items_center());
    let content = Stack::vertical((top, intro, providers, footer))
        .style(|s| s.width_full().max_width(570).gap(19).items_start());
    Scroll::new(content.container().style(|s| {
        s.width_full()
            .min_height_full()
            .padding(25)
            .items_center()
            .justify_center()
    }))
    .style(|s| s.size_full().min_size(0, 0).background(canvas()))
}
