use anyhow::{Context, Result};
use evdev::{
    uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode,
    RelativeAxisCode, UinputAbsSetup,
};
use proto::{char_to_key, Button, Input};

const ABS_MAX: i32 = 65535;
const KEY_LSHIFT: u16 = 42;

pub struct Injector { kb: VirtualDevice, ptr: VirtualDevice }

impl Injector {
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for c in 1..=248u16 { keys.insert(KeyCode::new(c)); }
        let kb = VirtualDevice::builder()
            .context("cannot open /dev/uinput (add yourself to the `input` group or fix its permissions)")?
            .name("pc-control keyboard")
            .with_keys(&keys)?
            .build()?;

        let mut btns = AttributeSet::<KeyCode>::new();
        for b in [KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE] { btns.insert(b); }
        let mut rel = AttributeSet::<RelativeAxisCode>::new();
        rel.insert(RelativeAxisCode::REL_WHEEL);
        rel.insert(RelativeAxisCode::REL_HWHEEL);
        let ax = |a| UinputAbsSetup::new(a, AbsInfo::new(0, 0, ABS_MAX, 0, 0, 1));
        let ptr = VirtualDevice::builder()?
            .name("pc-control pointer")
            .with_keys(&btns)?
            .with_relative_axes(&rel)?
            .with_absolute_axis(&ax(AbsoluteAxisCode::ABS_X))?
            .with_absolute_axis(&ax(AbsoluteAxisCode::ABS_Y))?
            .build()?;
        Ok(Self { kb, ptr })
    }

    pub fn handle(&mut self, i: Input) -> Result<()> {
        match i {
            Input::MoveTo { x, y } => {
                let p = |v: f32| (v.clamp(0.0, 1.0) * ABS_MAX as f32) as i32;
                self.ptr.emit(&[
                    InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, p(x)),
                    InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, p(y)),
                ])?;
            }
            Input::Button { button, down } => {
                let k = match button { Button::Left => KeyCode::BTN_LEFT, Button::Right => KeyCode::BTN_RIGHT, Button::Middle => KeyCode::BTN_MIDDLE };
                self.ptr.emit(&[InputEvent::new(EventType::KEY.0, k.0, down as i32)])?;
            }
            Input::Scroll { dx, dy } => {
                let mut ev = vec![];
                if dy != 0 { ev.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_WHEEL.0, dy.clamp(-20, 20))); }
                if dx != 0 { ev.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_HWHEEL.0, dx.clamp(-20, 20))); }
                if !ev.is_empty() { self.ptr.emit(&ev)?; }
            }
            Input::Key { code, down } => {
                if (1..=248).contains(&code) {
                    self.kb.emit(&[InputEvent::new(EventType::KEY.0, code, down as i32)])?;
                }
            }
            Input::Text(t) => {
                for c in t.chars().take(512) {
                    let Some((code, shift)) = char_to_key(c) else { continue };
                    let k = |code, v| InputEvent::new(EventType::KEY.0, code, v);
                    if shift { self.kb.emit(&[k(KEY_LSHIFT, 1)])?; }
                    self.kb.emit(&[k(code, 1)])?;
                    self.kb.emit(&[k(code, 0)])?;
                    if shift { self.kb.emit(&[k(KEY_LSHIFT, 0)])?; }
                }
            }
        }
        Ok(())
    }
}
