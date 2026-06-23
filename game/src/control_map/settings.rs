/*
 * control_map/settings.rs — parser and resource for Settings/pad.tune.
 */
use crate::filesystem::vfs;
use crate::oni2_loader::parsers::settings::{SettingsValue, parse_settings};
use bevy::prelude::*;

#[derive(Debug, Clone, Resource)]
pub struct PadTuneSettings {
    pub threshold: f32,
    pub direction_delay: f32,
    pub no_stored_cam: bool,
    pub direction_delay_after_atk: f32,
    pub digital_acts_as_analog: bool,
    pub lock_on_button_is_toggle: bool,
    pub lock_on_button_hold_is_on: bool,
    pub fight_mode_with_lockon: bool,
    pub crouch_button_is_toggle: bool,
    pub fight_hop_with_block: bool,
    pub lock_on_closest_90: bool,
    pub initial_strafe_time: f32,
    pub blend_strafe_time: f32,
    pub enemy_auto_trac_cull_range: f32,
    pub enemy_auto_trac_fudge: f32,
    pub enable_eatme: bool,
    pub throttle_yaw_dampening_factor: f32,
    pub bump_turn_time: f32,
    pub time_to_run_before_executing_running_atk: f32,
    pub use_eatme_for_bump_turn: bool,
    pub block_button_is_hold: bool,
    pub fight_mode_button_is_toggle: bool,
}

impl Default for PadTuneSettings {
    fn default() -> Self {
        Self {
            threshold: 0.1,
            direction_delay: 0.0,
            no_stored_cam: true,
            direction_delay_after_atk: 0.0,
            digital_acts_as_analog: false,
            lock_on_button_is_toggle: false,
            lock_on_button_hold_is_on: true,
            fight_mode_with_lockon: true,
            crouch_button_is_toggle: true,
            fight_hop_with_block: false,
            lock_on_closest_90: true,
            initial_strafe_time: 0.3,
            blend_strafe_time: 0.3,
            enemy_auto_trac_cull_range: 0.3,
            enemy_auto_trac_fudge: 45.0,
            enable_eatme: true,
            throttle_yaw_dampening_factor: 0.3,
            bump_turn_time: 0.15,
            time_to_run_before_executing_running_atk: 0.5,
            use_eatme_for_bump_turn: true,
            block_button_is_hold: false,
            fight_mode_button_is_toggle: false,
        }
    }
}

pub fn parse_pad_tune(content: &str) -> Result<PadTuneSettings, String> {
    let defs = parse_settings(content);
    let def = defs
        .first()
        .ok_or_else(|| "No definitions found in pad.tune".to_string())?;

    let pad_val = def
        .block
        .properties
        .get("Pad")
        .ok_or_else(|| "No 'Pad' block found in pad.tune".to_string())?;

    let pad_block = match pad_val {
        SettingsValue::Block(b) => b,
        _ => return Err("'Pad' is not a block".to_string()),
    };

    let mut settings = PadTuneSettings::default();

    let get_float = |key: &str| -> Option<f32> {
        match pad_block.properties.get(key) {
            Some(SettingsValue::Float(f)) => Some(*f),
            Some(SettingsValue::Int(i)) => Some(*i as f32),
            _ => None,
        }
    };

    let get_int = |key: &str| -> Option<i32> {
        match pad_block.properties.get(key) {
            Some(SettingsValue::Int(i)) => Some(*i),
            Some(SettingsValue::Float(f)) => Some(*f as i32),
            _ => None,
        }
    };

    let get_bool = |key: &str| -> Option<bool> { get_int(key).map(|i| i != 0) };

    if let Some(v) = get_float("threshold") {
        settings.threshold = v;
    }
    if let Some(v) = get_float("directiondelay") {
        settings.direction_delay = v;
    }
    if let Some(v) = get_bool("nostoredcam") {
        settings.no_stored_cam = v;
    }
    if let Some(v) = get_float("directiondelayafteratk") {
        settings.direction_delay_after_atk = v;
    }
    if let Some(v) = get_bool("digitalactsasanalog") {
        settings.digital_acts_as_analog = v;
    }
    if let Some(v) = get_bool("LockOnButtonIsToggle") {
        settings.lock_on_button_is_toggle = v;
    }
    if let Some(v) = get_bool("LockOnButtonHoldIsOn") {
        settings.lock_on_button_hold_is_on = v;
    }
    if let Some(v) = get_bool("FightModeWithLockon") {
        settings.fight_mode_with_lockon = v;
    }
    if let Some(v) = get_bool("CrouchButtonIsToggle") {
        settings.crouch_button_is_toggle = v;
    }
    if let Some(v) = get_bool("FightHopWithBlock") {
        settings.fight_hop_with_block = v;
    }
    if let Some(v) = get_bool("LockOnClosest90") {
        settings.lock_on_closest_90 = v;
    }
    if let Some(v) = get_float("InitialStrafeTime") {
        settings.initial_strafe_time = v;
    }
    if let Some(v) = get_float("BlendStrafeTime") {
        settings.blend_strafe_time = v;
    }
    if let Some(v) = get_float("EnemyAutoTracCullRange") {
        settings.enemy_auto_trac_cull_range = v;
    }
    if let Some(v) = get_float("EnemyAutoTracFudge") {
        settings.enemy_auto_trac_fudge = v;
    }
    if let Some(v) = get_bool("EnableEATME") {
        settings.enable_eatme = v;
    }
    if let Some(v) = get_float("ThrottleYawDampeningFactor") {
        settings.throttle_yaw_dampening_factor = v;
    }
    if let Some(v) = get_float("BumpTurnTime") {
        settings.bump_turn_time = v;
    }
    if let Some(v) = get_float("TimeToRunBeforeExecutingRunningAtk") {
        settings.time_to_run_before_executing_running_atk = v;
    }
    if let Some(v) = get_bool("UseEATMEforBumpTurn") {
        settings.use_eatme_for_bump_turn = v;
    }
    if let Some(v) = get_bool("BlockButtonIsHold") {
        settings.block_button_is_hold = v;
    }
    if let Some(v) = get_bool("FightModeButtonIsToggle") {
        settings.fight_mode_button_is_toggle = v;
    }

    Ok(settings)
}

pub fn load_pad_settings() -> PadTuneSettings {
    let content = if let Ok(s) = vfs::read_to_string("Settings", "pad.tune") {
        s
    } else {
        let paths = [
            "oni2/zips/assets/Settings/pad.tune",
            "assets/Settings/pad.tune",
        ];
        let mut loaded = None;
        for p in &paths {
            if let Ok(s) = std::fs::read_to_string(p) {
                loaded = Some(s);
                break;
            }
        }
        match loaded {
            Some(s) => s,
            None => {
                warn!("PadTune: could not find Settings/pad.tune; using defaults");
                return PadTuneSettings::default();
            }
        }
    };

    match parse_pad_tune(&content) {
        Ok(settings) => settings,
        Err(e) => {
            warn!("PadTune: parse error — {}. Using default settings.", e);
            PadTuneSettings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mock_pad_tune() {
        let content = r#"
type: a
Pad
{
	threshold 0.019999
	nostoredcam 1
	LockOnButtonIsToggle 0
	LockOnButtonHoldIsOn 1
	EnemyAutoTracCullRange 1.299995
	EnemyAutoTracFudge 120.000000
	EnableEATME 1
	BlockButtonIsHold 1
}
"#;
        let parsed = parse_pad_tune(content).unwrap();
        assert_eq!(parsed.threshold, 0.019999);
        assert!(parsed.no_stored_cam);
        assert!(!parsed.lock_on_button_is_toggle);
        assert!(parsed.lock_on_button_hold_is_on);
        assert_eq!(parsed.enemy_auto_trac_cull_range, 1.299995);
        assert_eq!(parsed.enemy_auto_trac_fudge, 120.0);
        assert!(parsed.enable_eatme);
        assert!(parsed.block_button_is_hold);
    }
}
