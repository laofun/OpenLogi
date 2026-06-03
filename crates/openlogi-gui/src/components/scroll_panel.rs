//! Scroll settings panel for the right-side config column.
//!
//! Exposes the software scroll pipeline's knobs: direction inversion, smoothing
//! toggles, and the four numeric tunables (speed / step / duration / dead zone).
//! Every edit is pushed live to the running hook via
//! [`crate::hook_runtime::push_scroll_settings`] and persisted to `config.toml`,
//! so the settings survive across launches and apply before this panel is ever
//! opened (see `main.rs`'s post-grant push).

use gpui::{
    AppContext as _, BorrowAppContext as _, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window, div, px,
};
use gpui_component::{
    h_flex,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use openlogi_core::config::ScrollSettings;

use crate::hook_runtime;
use crate::theme;

/// Slider column width. Matches the right-column layout in `app.rs`.
const PANEL_W: f32 = 300.;

pub struct ScrollPanel {
    settings: ScrollSettings,
    speed: Entity<SliderState>,
    step: Entity<SliderState>,
    duration: Entity<SliderState>,
    dead_zone: Entity<SliderState>,
    #[allow(dead_code, reason = "held to keep the four slider subscriptions alive")]
    _subs: Vec<Subscription>,
}

impl ScrollPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = openlogi_core::config::Config::load_or_default()
            .map(|c| c.scroll_settings())
            .unwrap_or_default();

        // Order matters: `SliderState` clamps the value against the current max,
        // so set `.max()` before `.min()` (matches `dpi_panel`).
        let speed = cx.new(|_| {
            SliderState::new()
                .max(10.0)
                .min(1.0)
                .step(0.1)
                .default_value(f64_to_f32(settings.speed))
        });
        let step = cx.new(|_| {
            SliderState::new()
                .max(100.0)
                .min(0.01)
                .step(0.5)
                .default_value(f64_to_f32(settings.step))
        });
        let duration = cx.new(|_| {
            SliderState::new()
                .max(5.0)
                .min(1.0)
                .step(0.05)
                .default_value(f64_to_f32(settings.duration))
        });
        let dead_zone = cx.new(|_| {
            SliderState::new()
                .max(10.0)
                .min(f64_to_f32(openlogi_hook::MIN_DEAD_ZONE))
                .step(0.1)
                .default_value(f64_to_f32(settings.dead_zone))
        });

        // Subscribe to `Release` only (not `Change`) so a drag doesn't spam the
        // hook + config persistence on every intermediate frame.
        let mut subs = Vec::with_capacity(4);
        subs.push(
            cx.subscribe(&speed, |this, _slider, event: &SliderEvent, cx| {
                if let SliderEvent::Release(v) = event {
                    this.settings.speed = f64::from(v.start());
                    this.on_change(cx);
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe(&step, |this, _slider, event: &SliderEvent, cx| {
                if let SliderEvent::Release(v) = event {
                    this.settings.step = f64::from(v.start());
                    this.on_change(cx);
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe(&duration, |this, _slider, event: &SliderEvent, cx| {
                if let SliderEvent::Release(v) = event {
                    this.settings.duration = f64::from(v.start());
                    this.on_change(cx);
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe(&dead_zone, |this, _slider, event: &SliderEvent, cx| {
                if let SliderEvent::Release(v) = event {
                    this.settings.dead_zone = f64::from(v.start());
                    this.on_change(cx);
                    cx.notify();
                }
            }),
        );

        Self {
            settings,
            speed,
            step,
            duration,
            dead_zone,
            _subs: subs,
        }
    }

    /// Push the current settings to the live hook and persist them to disk.
    fn on_change(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        // Live push to the running hook (lock-free ArcSwap).
        hook_runtime::push_scroll_settings(settings.clone());
        // Persist through AppState's in-memory config so other full-file saves
        // (device select, DPI presets) can't revert these edits.
        cx.update_global::<crate::state::AppState, _>(move |state, _| {
            state.commit_scroll_settings(settings);
        });
    }

    /// One labelled boolean toggle row. `set` writes the toggled value back into
    /// `ScrollSettings`, then [`Self::on_change`] applies + persists it.
    fn toggle_row(
        id: &'static str,
        label: SharedString,
        checked: bool,
        set: fn(&mut ScrollSettings, bool),
        pal: theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .justify_between()
            .items_center()
            .child(div().text_sm().text_color(pal.text_primary).child(label))
            .child(Switch::new(id).checked(checked).on_click(cx.listener(
                move |this, checked: &bool, _window, cx| {
                    set(&mut this.settings, *checked);
                    this.on_change(cx);
                    cx.notify();
                },
            )))
    }
}

impl Render for ScrollPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);

        v_flex()
            .gap_3()
            .w(px(PANEL_W))
            .child(div().text_sm().text_color(pal.text_muted).child("SCROLL"))
            // Toggles
            .child(Self::toggle_row(
                "scroll-smooth",
                tr!("Smooth scrolling"),
                self.settings.smooth,
                |s, v| s.smooth = v,
                pal,
                cx,
            ))
            .child(Self::toggle_row(
                "scroll-invert-v",
                tr!("Invert wheel (vertical)"),
                self.settings.reverse_vertical,
                |s, v| s.reverse_vertical = v,
                pal,
                cx,
            ))
            .child(Self::toggle_row(
                "scroll-invert-h",
                tr!("Invert thumb wheel (horizontal)"),
                self.settings.reverse_horizontal,
                |s, v| s.reverse_horizontal = v,
                pal,
                cx,
            ))
            .child(Self::toggle_row(
                "scroll-smooth-v",
                tr!("Smooth vertical"),
                self.settings.smooth_vertical,
                |s, v| s.smooth_vertical = v,
                pal,
                cx,
            ))
            .child(Self::toggle_row(
                "scroll-smooth-h",
                tr!("Smooth horizontal"),
                self.settings.smooth_horizontal,
                |s, v| s.smooth_horizontal = v,
                pal,
                cx,
            ))
            // Sliders
            .child(slider_row(tr!("Speed"), self.settings.speed, &self.speed, pal))
            .child(slider_row(tr!("Step"), self.settings.step, &self.step, pal))
            .child(slider_row(
                tr!("Duration"),
                self.settings.duration,
                &self.duration,
                pal,
            ))
            .child(slider_row(
                tr!("Dead zone"),
                self.settings.dead_zone,
                &self.dead_zone,
                pal,
            ))
    }
}

/// One labelled slider row: a header showing the label and the current value,
/// then the slider itself underneath.
fn slider_row(
    label: SharedString,
    value: f64,
    state: &Entity<SliderState>,
    pal: theme::Palette,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .child(div().text_sm().text_color(pal.text_muted).child(label))
                .child(
                    div()
                        .text_sm()
                        .text_color(pal.text_muted)
                        .child(format!("{value:.2}")),
                ),
        )
        .child(Slider::new(state).horizontal())
}

/// Narrow a small, bounded scroll knob into f32 for slider math.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "scroll knobs are small bounded values; f32 slider precision is ample"
)]
fn f64_to_f32(v: f64) -> f32 {
    v as f32
}
