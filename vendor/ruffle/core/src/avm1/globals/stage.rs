//! Stage object
//!
//! TODO: This is a very rough stub with not much implementation.

use crate::avm1::activation::Activation;
use crate::avm1::error::Error;
use crate::avm1::globals::as_broadcaster::BroadcasterFunctions;
use crate::avm1::globals::movie_clip::new_rectangle;
use crate::avm1::property_decl::{DeclContext, StaticDeclarations};
use crate::avm1::{Object, Value};
use crate::display_object::StageDisplayState;
use crate::string::{AvmString, WStr, WString};
use ruffle_macros::istr;
use swf::{Rectangle, Twips};

const OBJECT_DECLS: StaticDeclarations = declare_static_properties! {
    "align" => property(align, set_align);
    "scaleMode" => property(scale_mode, set_scale_mode);
    "height" => property(height);
    "width" => property(width);
    "showMenu" => property(show_menu, set_show_menu);
    "displayState" => property(display_state, set_display_state);
    "visibleRect" => property(visible_rect);
    "safeRect" => property(safe_rect);
};

pub fn create<'gc>(
    context: &mut DeclContext<'_, 'gc>,
    broadcaster_functions: BroadcasterFunctions<'gc>,
    array_proto: Object<'gc>,
) -> Object<'gc> {
    let stage = Object::new(context.strings, Some(context.object_proto));
    broadcaster_functions.initialize(context.strings, stage, array_proto);
    context.define_properties_on(stage, OBJECT_DECLS(context));
    stage
}

fn align<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let align = activation.context.stage.align();
    let mut s = WString::with_capacity(4, false);
    // Match string values returned by AS.
    // It's possible to have an oxymoronic "LTRB".
    // This acts the same as "TL" (top-left takes priority).
    // This order is different between AVM1 and AVM2!
    use crate::display_object::StageAlign;
    if align.contains(StageAlign::LEFT) {
        s.push_byte(b'L');
    }
    if align.contains(StageAlign::TOP) {
        s.push_byte(b'T');
    }
    if align.contains(StageAlign::RIGHT) {
        s.push_byte(b'R');
    }
    if align.contains(StageAlign::BOTTOM) {
        s.push_byte(b'B');
    }
    let align = AvmString::new(activation.gc(), s);
    Ok(align.into())
}

fn set_align<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let align = args
        .get(0)
        .unwrap_or(&Value::Undefined)
        .coerce_to_string(activation)?
        .parse()
        .unwrap_or_default();
    activation
        .context
        .stage
        .set_align(activation.context, align);
    Ok(Value::Undefined)
}

fn height<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    Ok(activation.context.stage.stage_size().1.into())
}

fn scale_mode<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let scale_mode = AvmString::new_utf8(
        activation.gc(),
        activation.context.stage.scale_mode().to_avm_string(),
    );
    Ok(scale_mode.into())
}

fn set_scale_mode<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let scale_mode = args
        .get(0)
        .unwrap_or(&Value::Undefined)
        .coerce_to_string(activation)?
        .parse()
        .unwrap_or_default();
    activation
        .context
        .stage
        .set_scale_mode(activation.context, scale_mode, true);
    Ok(Value::Undefined)
}

fn display_state<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let state = if activation.context.stage.is_fullscreen() {
        istr!("fullScreen")
    } else {
        istr!("normal")
    };

    Ok(state.into())
}

fn set_display_state<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let display_state = args
        .get(0)
        .unwrap_or(&Value::Undefined)
        .coerce_to_string(activation)?;

    if display_state.eq_ignore_case(WStr::from_units(b"fullscreen")) {
        activation
            .context
            .stage
            .set_display_state(activation.context, StageDisplayState::FullScreen);
    } else if display_state.eq_ignore_case(WStr::from_units(b"normal")) {
        activation
            .context
            .stage
            .set_display_state(activation.context, StageDisplayState::Normal);
    }

    Ok(Value::Undefined)
}

fn show_menu<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    Ok(activation.context.stage.show_menu().into())
}

fn set_show_menu<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let show_menu = args
        .get(0)
        .unwrap_or(&true.into())
        .to_owned()
        .as_bool(activation.swf_version());
    activation.context.stage.set_show_menu(show_menu);
    Ok(Value::Undefined)
}

fn width<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    Ok(activation.context.stage.stage_size().0.into())
}

/// DOVAHENGINE PATCH (M2): `Stage.visibleRect` / `Stage.safeRect`
/// (Scaleform GFx extensions, not in stock Flash/Ruffle).
///
/// Skyrim's HUD lays itself out with `GlobalFunc.Lock`, which reads
/// `Stage.visibleRect` and `Stage.safeRect` to anchor the meters,
/// compass, and location name to the stage edges. With both
/// properties undefined, `Lock` computes NaN anchors; Ruffle's
/// `globalToLocal` coerces NaN to 0 twips (Rust `as i32` saturating
/// cast), so every anchored clip parks at global (0, 0) — the meter
/// art hangs above its clip origin and lands off the top of the
/// screen, and the location text lands off the left edge. Only the
/// compass (art below its origin) stays visible. Returning the real
/// stage rect restores the game's own layout.
///
/// `visibleRect` is the full stage in stage coordinates.
/// `safeRect` is the same rect while there is no safe-area inset —
/// the phone build can narrow it for notches/gesture bars later.
fn visible_rect<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let (width, height) = activation.context.stage.stage_size();
    new_rectangle(
        activation,
        Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels_i32(width as i32),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels_i32(height as i32),
        },
    )
}

fn safe_rect<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    // The safe rect is the stage rect minus the host's safe-area
    // insets (notch/gesture bar — Dovahengine patch 0007), clamped
    // so it never inverts. Zero insets (desktop) = the full stage.
    let (width, height) = activation.context.stage.stage_size();
    let (left, top, right, bottom) = activation.context.stage.safe_area_insets();
    let x_min = left.min(width);
    let y_min = top.min(height);
    let x_max = width.saturating_sub(right).max(x_min);
    let y_max = height.saturating_sub(bottom).max(y_min);
    new_rectangle(
        activation,
        Rectangle {
            x_min: Twips::from_pixels_i32(x_min as i32),
            x_max: Twips::from_pixels_i32(x_max as i32),
            y_min: Twips::from_pixels_i32(y_min as i32),
            y_max: Twips::from_pixels_i32(y_max as i32),
        },
    )
}
