use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, BorrowAppContext as _, BoxShadow, Context, Entity,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement as _, Styled, Subscription, Window, div, ease_in_out, point,
    prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{Icon, IconName, h_flex, v_flex};
use openlogi_core::device::{BatteryInfo, BatteryLevel, BatteryStatus, DeviceKind};

use crate::state::{AppState, DeviceRecord};
use crate::theme::{self, ACCENT_BLUE, Palette, STATUS_CONNECTED, STATUS_OFFLINE};

const CARD_W: f32 = 220.;
const CARD_H: f32 = 64.;
const DOT_SIZE: f32 = 10.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Connected,
    Offline,
}

impl Status {
    fn color(self) -> u32 {
        match self {
            Status::Connected => STATUS_CONNECTED,
            Status::Offline => STATUS_OFFLINE,
        }
    }
}

#[derive(Clone)]
struct CardData {
    name: String,
    sub: String,
    status: Status,
    battery: Option<BatteryInfo>,
}

/// Header carousel for paired devices.
///
/// Renders straight from [`AppState::device_list`] and re-renders whenever
/// that global changes (hot-plug, selection), so a device that connects after
/// launch shows up without a restart. Holds no card snapshot of its own.
pub struct DeviceCarousel {
    _state_obs: Subscription,
}

impl DeviceCarousel {
    /// Build the carousel and subscribe to device-list changes.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state_obs = cx.observe_global::<AppState>(|_, cx| cx.notify());
        Self {
            _state_obs: state_obs,
        }
    }
}

impl Render for DeviceCarousel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let pal = theme::palette(cx);
        let (cards, selected) = cx.try_global::<AppState>().map_or_else(
            || (Vec::new(), 0),
            |s| {
                let cards: Vec<CardData> = s.device_list.iter().map(card_from_record).collect();
                let selected = s.current_device.min(cards.len().saturating_sub(1));
                (cards, selected)
            },
        );

        h_flex()
            .id("device-carousel")
            .gap_3()
            .items_center()
            .overflow_x_scroll()
            .children(
                cards
                    .iter()
                    .enumerate()
                    .map(|(idx, card)| card_view(idx, card, idx == selected, &entity, pal)),
            )
    }
}

fn card_view(
    idx: usize,
    card: &CardData,
    selected: bool,
    entity: &Entity<DeviceCarousel>,
    pal: Palette,
) -> AnyElement {
    let battery = card.battery.clone();
    let entity = entity.clone();

    div()
        .id(("device-card", idx))
        .w(px(CARD_W))
        .h(px(CARD_H))
        .px_3()
        .py_2()
        .rounded_md()
        .border_2()
        .border_color(if selected {
            rgb(ACCENT_BLUE).into()
        } else {
            pal.border
        })
        .bg(pal.surface)
        .hover(|s| s.bg(pal.surface_hover))
        .on_click(move |_event, _window, cx| {
            cx.update_global::<AppState, _>(|state, _| state.set_current_device(idx));
            entity.update(cx, |_, cx| cx.notify());
        })
        .child(
            h_flex()
                .size_full()
                .gap_3()
                .items_center()
                .child(status_dot(card.status))
                .child(
                    v_flex()
                        .gap_0p5()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(card.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(pal.text_muted)
                                .child(card.sub.clone()),
                        ),
                )
                .when_some(battery, |this, b| this.child(battery_view(&b, pal))),
        )
        .into_any_element()
}

fn status_dot(status: Status) -> AnyElement {
    let base = div()
        .size(px(DOT_SIZE))
        .rounded_full()
        .bg(rgb(status.color()));
    match status {
        Status::Offline => base.into_any_element(),
        Status::Connected => base
            .with_animation(
                "status-breath",
                Animation::new(Duration::from_millis(2200))
                    .repeat()
                    .with_easing(ease_in_out),
                |this, delta| {
                    let intensity = (delta * std::f32::consts::PI).sin();
                    this.shadow(vec![BoxShadow {
                        color: gpui::hsla(0.35, 0.7, 0.55, 0.35 + intensity * 0.45),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(2. + intensity * 8.),
                        spread_radius: px(0.5),
                    }])
                },
            )
            .into_any_element(),
    }
}

fn card_from_record(r: &DeviceRecord) -> CardData {
    CardData {
        name: r.display_name.clone(),
        sub: format!("{} · slot {}", kind_label(r.kind), r.slot),
        status: if r.online {
            Status::Connected
        } else {
            Status::Offline
        },
        battery: r.battery.clone(),
    }
}

/// Battery readout for a device card: a charge/level icon plus the percentage,
/// both in the muted footer style.
fn battery_view(b: &BatteryInfo, pal: Palette) -> AnyElement {
    h_flex()
        .gap_1()
        .items_center()
        .text_xs()
        .text_color(pal.text_muted)
        .child(Icon::new(battery_icon(b)).size_3())
        .child(b.percentage_display())
        .into_any_element()
}

/// Pick the battery glyph from charge state first (charging / full / error),
/// then fall back to the discrete charge level for a plain discharge.
fn battery_icon(b: &BatteryInfo) -> IconName {
    match b.status {
        BatteryStatus::Charging | BatteryStatus::ChargingSlow => IconName::BatteryCharging,
        BatteryStatus::Full => IconName::BatteryFull,
        BatteryStatus::Error => IconName::BatteryWarning,
        BatteryStatus::Discharging | BatteryStatus::Unknown => match b.level {
            BatteryLevel::Critical => IconName::BatteryWarning,
            BatteryLevel::Low => IconName::BatteryLow,
            BatteryLevel::Good => IconName::BatteryMedium,
            BatteryLevel::Full => IconName::BatteryFull,
            BatteryLevel::Unknown => IconName::Battery,
        },
    }
}

fn kind_label(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Mouse => "Mouse",
        DeviceKind::Keyboard => "Keyboard",
        DeviceKind::Numpad => "Numpad",
        DeviceKind::Presenter => "Presenter",
        DeviceKind::Remote => "Remote",
        DeviceKind::Trackball => "Trackball",
        DeviceKind::Touchpad => "Touchpad",
        DeviceKind::Tablet => "Tablet",
        DeviceKind::Gamepad => "Gamepad",
        DeviceKind::Joystick => "Joystick",
        DeviceKind::Headset => "Headset",
        DeviceKind::Unknown => "Device",
    }
}
