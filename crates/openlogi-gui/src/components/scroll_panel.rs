//! Scroll settings panel for the right-side config column.
//!
//! Exposes the software scroll pipeline's knobs: per-axis inversion/smoothing,
//! the shared smoothing tunables, and active-device SmartShift controls.
//! Every scroll edit is pushed live to the running hook via
//! [`crate::hook_runtime::push_scroll_settings`] and persisted to `config.toml`.

use gpui::{
    AppContext as _, BorrowAppContext as _, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable, h_flex,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use openlogi_core::config::ScrollSettings;
use openlogi_hid::{DeviceRoute, SmartShiftMode};

use crate::hardware;
use crate::hook_runtime;
use crate::state::{AppState, SmartShiftState};
use crate::theme;

/// Slider column width. Matches the right-column layout in `app.rs`.
const PANEL_W: f32 = 300.;
/// Default raw SmartShift sensitivity used when the active device has no
/// persisted value yet.
const DEFAULT_SMARTSHIFT_RAW: u8 = 25;

pub struct ScrollPanel {
    settings: ScrollSettings,
    vertical_speed: Entity<SliderState>,
    horizontal_speed: Entity<SliderState>,
    vertical_step: Entity<SliderState>,
    horizontal_step: Entity<SliderState>,
    duration: Entity<SliderState>,
    dead_zone: Entity<SliderState>,
    smartshift_sensitivity: Entity<SliderState>,
    _state_obs: Subscription,
    #[allow(dead_code, reason = "held to keep slider subscriptions alive")]
    _subs: Vec<Subscription>,
}

impl ScrollPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = openlogi_core::config::Config::load_or_default()
            .map(|c| c.scroll_settings())
            .unwrap_or_default();
        let state_obs = cx.observe_global::<AppState>(|_this, cx| {
            cx.notify();
        });

        let vertical_speed = cx.new(|_| speed_slider_state(settings.vertical_speed));
        let horizontal_speed = cx.new(|_| speed_slider_state(settings.horizontal_speed));
        let vertical_step = cx.new(|_| step_slider_state(settings.vertical_step));
        let horizontal_step = cx.new(|_| step_slider_state(settings.horizontal_step));
        let duration = cx.new(|_| duration_slider_state(settings.duration));
        let dead_zone = cx.new(|_| dead_zone_slider_state(settings.dead_zone));
        // Seeded to the default; `sync_smartshift_slider` re-seats it from the
        // device's live state once the lazy read lands (Ready). The device is
        // never driven from this initial value — it's display-only until Ready.
        let smartshift_sensitivity =
            cx.new(|_| smartshift_slider_state(f32::from(default_smartshift_percent())));

        let mut subs = Vec::with_capacity(7);
        subs.push(subscribe_scroll_slider(
            cx,
            &vertical_speed,
            |settings, value| settings.vertical_speed = f64::from(value),
        ));
        subs.push(subscribe_scroll_slider(
            cx,
            &horizontal_speed,
            |settings, value| settings.horizontal_speed = f64::from(value),
        ));
        subs.push(subscribe_scroll_slider(
            cx,
            &vertical_step,
            |settings, value| settings.vertical_step = f64::from(value),
        ));
        subs.push(subscribe_scroll_slider(
            cx,
            &horizontal_step,
            |settings, value| settings.horizontal_step = f64::from(value),
        ));
        subs.push(subscribe_scroll_slider(cx, &duration, |settings, value| {
            settings.duration = f64::from(value);
        }));
        subs.push(subscribe_scroll_slider(
            cx,
            &dead_zone,
            |settings, value| settings.dead_zone = f64::from(value),
        ));
        subs.push(cx.subscribe(
            &smartshift_sensitivity,
            |_this, _slider, event: &SliderEvent, cx| {
                if let SliderEvent::Release(value) = event {
                    Self::apply_smartshift_sensitivity_release(value.start(), cx);
                }
            },
        ));

        Self {
            settings,
            vertical_speed,
            horizontal_speed,
            vertical_step,
            horizontal_step,
            duration,
            dead_zone,
            smartshift_sensitivity,
            _state_obs: state_obs,
            _subs: subs,
        }
    }

    fn on_change(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        hook_runtime::push_scroll_settings(settings.clone());
        cx.update_global::<AppState, _>(move |state, _| {
            state.commit_scroll_settings(settings);
        });
    }

    fn reset_to_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = ScrollSettings::default();
        let thumbs = [
            (&self.vertical_speed, defaults.vertical_speed),
            (&self.horizontal_speed, defaults.horizontal_speed),
            (&self.vertical_step, defaults.vertical_step),
            (&self.horizontal_step, defaults.horizontal_step),
            (&self.duration, defaults.duration),
            (&self.dead_zone, defaults.dead_zone),
        ];
        for (state, value) in thumbs {
            state.update(cx, |s, cx| s.set_value(f64_to_f32(value), &mut *window, cx));
        }
        self.settings = defaults;
        self.on_change(cx);
        cx.notify();
    }

    /// Kick off a one-shot SmartShift status read for the active device when it
    /// hasn't been queried yet. Mirrors [`crate::components::dpi_panel`]'s
    /// `ensure_dpi_load`: triggered from `render`, runs the blocking HID++ read
    /// on a dedicated OS thread, and stores the result back on the global. This
    /// is the read-only path — the device keeps whatever mode it powered up in;
    /// the UI just mirrors it.
    fn ensure_smartshift_load(cx: &mut Context<Self>) {
        let Some((key, route)) = smartshift_load_target(cx) else {
            return;
        };

        cx.update_global::<AppState, _>(|state, _| state.mark_smartshift_loading(&key));
        let key_for_reset = key.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = hardware::read_smartshift_status_blocking(&route);
            let _ = tx.send((key, route, result));
        });
        cx.spawn(async move |_panel, cx| {
            match rx.await {
                Ok((key, route, result)) => {
                    cx.update_global::<AppState, _>(|state, cx| {
                        state.store_smartshift_status(key, &route, result);
                        cx.refresh_windows();
                    });
                }
                // The worker vanished without sending (e.g. it panicked). Reset
                // the `Loading` marker so the device isn't stuck on "reading…".
                Err(_) => {
                    cx.update_global::<AppState, _>(|state, cx| {
                        state.clear_smartshift_loading(&key_for_reset);
                        cx.refresh_windows();
                    });
                }
            }
        })
        .detach();
    }

    fn sync_smartshift_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let percent = smartshift_percent_from_state(cx);
        self.smartshift_sensitivity.update(cx, |state, cx| {
            let target = f32::from(percent);
            if smartshift_slider_needs_sync(state.value().start(), target) {
                state.set_value(target, &mut *window, cx);
            }
        });
    }

    fn apply_smartshift_sensitivity_release(percent: f32, cx: &mut Context<Self>) {
        let (ratchet_mode, target, _percent, ready) = smartshift_render_snapshot(cx);
        if !ready || !ratchet_mode {
            // Not interactive yet (device unread) or sensitivity does not apply
            // in Free mode; re-seat the thumb from live state on the next render.
            cx.notify();
            return;
        }

        let percent = round_slider_percent(percent);
        let raw = hardware::smartshift_percent_to_raw(percent);
        cx.update_global::<AppState, _>(|state, _| {
            state.set_active_smartshift_sensitivity_optimistic(raw);
        });
        hardware::apply_smartshift_sensitivity_in_background(target, raw);
        cx.notify();
    }

    fn set_ratchet_mode(enabled: bool, cx: &mut Context<Self>) {
        let (_ratchet_mode, target, _percent, ready) = smartshift_render_snapshot(cx);
        if !ready {
            // The toggle is only interactive once the live state is known.
            cx.notify();
            return;
        }
        let mode = if enabled {
            SmartShiftMode::Ratchet
        } else {
            SmartShiftMode::Free
        };
        cx.update_global::<AppState, _>(|state, _| {
            state.set_active_smartshift_mode_optimistic(mode);
        });
        hardware::set_smartshift_mode_in_background(None, target, mode);
        cx.notify();
    }

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
    #[allow(
        clippy::too_many_lines,
        reason = "right-column settings panel composes several small grouped sections inline"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Self::ensure_smartshift_load(cx);
        self.sync_smartshift_slider(window, cx);
        let pal = theme::palette(cx);
        let (ratchet_mode, target, smartshift_percent, ready) = smartshift_render_snapshot(cx);
        // The toggle is interactive once the device's live state is known; the
        // sensitivity slider additionally requires Ratchet mode to be on.
        let toggle_disabled = !ready || target.is_none();
        let smartshift_disabled = toggle_disabled || !ratchet_mode;

        v_flex()
            .gap_4()
            .w(px(PANEL_W))
            .child(div().text_sm().text_color(pal.text_muted).child("SCROLL"))
            .child(section(
                tr!("VERTICAL WHEEL"),
                v_flex()
                    .gap_3()
                    .child(Self::toggle_row(
                        "scroll-invert-v",
                        tr!("Invert vertical"),
                        self.settings.reverse_vertical,
                        |s, v| s.reverse_vertical = v,
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
                    .child(slider_row(
                        tr!("Speed"),
                        self.settings.vertical_speed,
                        &self.vertical_speed,
                        pal,
                        false,
                    ))
                    .child(slider_row(
                        tr!("Step"),
                        self.settings.vertical_step,
                        &self.vertical_step,
                        pal,
                        false,
                    )),
                pal,
            ))
            .child(section(
                tr!("THUMB WHEEL / HORIZONTAL"),
                v_flex()
                    .gap_3()
                    .child(Self::toggle_row(
                        "scroll-invert-h",
                        tr!("Invert horizontal"),
                        self.settings.reverse_horizontal,
                        |s, v| s.reverse_horizontal = v,
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
                    .child(slider_row(
                        tr!("Speed"),
                        self.settings.horizontal_speed,
                        &self.horizontal_speed,
                        pal,
                        false,
                    ))
                    .child(slider_row(
                        tr!("Step"),
                        self.settings.horizontal_step,
                        &self.horizontal_step,
                        pal,
                        false,
                    )),
                pal,
            ))
            .child(section(
                tr!("SMOOTH FEEL"),
                v_flex()
                    .gap_3()
                    .child(Self::toggle_row(
                        "scroll-smooth",
                        tr!("Smooth scrolling"),
                        self.settings.smooth,
                        |s, v| s.smooth = v,
                        pal,
                        cx,
                    ))
                    .child(slider_row(
                        tr!("Duration"),
                        self.settings.duration,
                        &self.duration,
                        pal,
                        false,
                    ))
                    .child(slider_row(
                        tr!("Dead zone"),
                        self.settings.dead_zone,
                        &self.dead_zone,
                        pal,
                        false,
                    )),
                pal,
            ))
            .child(section(
                tr!("SMARTSHIFT"),
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(pal.text_primary)
                                    .child(tr!("Ratchet mode")),
                            )
                            .child(
                                Switch::new("smartshift-ratchet")
                                    .checked(ratchet_mode)
                                    .disabled(toggle_disabled)
                                    .on_click(cx.listener(|_this, checked: &bool, _window, cx| {
                                        Self::set_ratchet_mode(*checked, cx);
                                    })),
                            ),
                    )
                    .child(slider_row(
                        tr!("SmartShift sensitivity"),
                        f64::from(smartshift_percent),
                        &self.smartshift_sensitivity,
                        pal,
                        smartshift_disabled,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(pal.text_muted)
                            .child(tr!("Sensitivity applies only while Ratchet mode is on.")),
                    ),
                pal,
            ))
            .child(
                div()
                    .id("scroll-reset")
                    .text_sm()
                    .text_color(pal.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(pal.text_primary))
                    .child(tr!("Reset to defaults"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.reset_to_defaults(window, cx);
                    })),
            )
    }
}

fn section(
    title: SharedString,
    content: impl IntoElement,
    pal: theme::Palette,
) -> impl IntoElement {
    v_flex()
        .gap_2()
        .child(div().text_xs().text_color(pal.text_muted).child(title))
        .child(content)
}

fn slider_row(
    label: SharedString,
    value: f64,
    state: &Entity<SliderState>,
    pal: theme::Palette,
    disabled: bool,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .when(disabled, |s| s.opacity(0.45))
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

fn subscribe_scroll_slider(
    cx: &mut Context<ScrollPanel>,
    state: &Entity<SliderState>,
    apply: fn(&mut ScrollSettings, f32),
) -> Subscription {
    cx.subscribe(state, move |this, _slider, event: &SliderEvent, cx| {
        if let SliderEvent::Release(value) = event {
            apply(&mut this.settings, value.start());
            this.on_change(cx);
            cx.notify();
        }
    })
}

fn speed_slider_state(value: f64) -> SliderState {
    SliderState::new()
        .max(10.0)
        .min(1.0)
        .step(0.1)
        .default_value(f64_to_f32(value))
}

fn step_slider_state(value: f64) -> SliderState {
    SliderState::new()
        .max(100.0)
        .min(0.01)
        .step(0.5)
        .default_value(f64_to_f32(value))
}

fn duration_slider_state(value: f64) -> SliderState {
    SliderState::new()
        .max(5.0)
        .min(1.0)
        .step(0.05)
        .default_value(f64_to_f32(value))
}

fn dead_zone_slider_state(value: f64) -> SliderState {
    SliderState::new()
        .max(10.0)
        .min(f64_to_f32(openlogi_hook::MIN_DEAD_ZONE))
        .step(0.1)
        .default_value(f64_to_f32(value))
}

fn smartshift_slider_state(value: f32) -> SliderState {
    SliderState::new()
        .max(100.0)
        .min(0.0)
        .step(1.0)
        .default_value(value)
}

/// The active device's SmartShift sensitivity as a slider percentage, derived
/// from its live state. Falls back to the default until the device reports
/// `Ready` (Unknown / Loading / Failed).
fn smartshift_percent_from_state(cx: &mut Context<ScrollPanel>) -> u8 {
    match cx
        .try_global::<AppState>()
        .map(AppState::current_smartshift_status)
    {
        Some(SmartShiftState::Ready { sensitivity, .. }) => {
            hardware::smartshift_raw_to_percent(sensitivity)
        }
        _ => default_smartshift_percent(),
    }
}

fn default_smartshift_percent() -> u8 {
    hardware::smartshift_raw_to_percent(DEFAULT_SMARTSHIFT_RAW)
}

/// `(ratchet_mode, target_route, sensitivity_percent, ready)` for the active
/// device, read from its live SmartShift state. `ready` is true only once the
/// device has reported its mode + sensitivity; until then the toggle shows off
/// and both controls are disabled.
fn smartshift_render_snapshot(
    cx: &mut Context<ScrollPanel>,
) -> (bool, Option<DeviceRoute>, u8, bool) {
    let target = cx.try_global::<AppState>().and_then(|state| {
        state
            .current_record()
            .and_then(|record| record.route.clone())
    });
    let status = cx.try_global::<AppState>().map_or(
        SmartShiftState::Unknown,
        AppState::current_smartshift_status,
    );
    match status {
        SmartShiftState::Ready { mode, sensitivity } => (
            mode == SmartShiftMode::Ratchet,
            target,
            hardware::smartshift_raw_to_percent(sensitivity),
            true,
        ),
        SmartShiftState::Unknown | SmartShiftState::Loading | SmartShiftState::Failed(_) => {
            (false, target, default_smartshift_percent(), false)
        }
    }
}

/// The active device's `(config_key, route)` when it still needs a SmartShift
/// read — i.e. its live state is `Unknown`. Mirrors `dpi_panel::dpi_load_target`.
fn smartshift_load_target(cx: &mut Context<ScrollPanel>) -> Option<(String, DeviceRoute)> {
    cx.try_global::<AppState>().and_then(|state| {
        if !state.current_smartshift_unqueried() {
            return None;
        }
        let record = state.current_record()?;
        Some((record.config_key.clone(), record.route.clone()?))
    })
}

fn smartshift_slider_needs_sync(current: f32, target: f32) -> bool {
    (current - target).abs() > f32::EPSILON
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "SmartShift slider is constrained to integer percentage values"
)]
fn round_slider_percent(value: f32) -> u8 {
    value.round().clamp(0.0, 100.0) as u8
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "scroll knobs are small bounded values; f32 slider precision is ample"
)]
fn f64_to_f32(v: f64) -> f32 {
    v as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_smartshift_percent_uses_default_raw_value() {
        assert_eq!(default_smartshift_percent(), 9);
    }

    #[test]
    fn round_slider_percent_clamps_and_rounds() {
        assert_eq!(round_slider_percent(-3.2), 0);
        assert_eq!(round_slider_percent(18.6), 19);
        assert_eq!(round_slider_percent(200.0), 100);
    }

    #[test]
    fn smartshift_slider_sync_only_when_percent_changes() {
        assert!(!smartshift_slider_needs_sync(42.0, 42.0));
        assert!(smartshift_slider_needs_sync(41.0, 42.0));
    }
}
