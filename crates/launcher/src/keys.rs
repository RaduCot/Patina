//! Win-key decisions, kept free of Win32 so real key sequences can be tested.
//! The hook in signal.rs feeds every (non-Patina-injected) key through
//! `KeyState::on_key` and carries out the returned action.

pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_S: u32 = 0x53;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Action {
    /// Let Windows have the key.
    Pass,
    /// Eat the key.
    Swallow,
    /// A lone Win tap: eat the Win-up, inject mask down/up plus that Win-up
    /// (so the shell sees a combo and keeps Start shut), open Patina.
    WinTap,
    /// Win+S: eat the S, inject mask down/up, open Patina.
    WinS,
    /// Ctrl+Esc: pass it, and let the real Start open this once (Patina
    /// otherwise closes Start whenever it appears).
    RealStart,
}

const VK_ESCAPE: u32 = 0x1B;

/// Shift / Ctrl / Alt, generic and left/right (the low-level hook reports L/R).
fn modifier_bit(vk: u32) -> u8 {
    match vk {
        0x10 | 0xA0 | 0xA1 => 1, // Shift
        0x11 | 0xA2 | 0xA3 => 2, // Ctrl
        0x12 | 0xA4 | 0xA5 => 4, // Alt
        _ => 0,
    }
}

#[derive(Default)]
pub struct KeyState {
    win_down: bool,
    /// Something else happened while Win was held (or a modifier was already
    /// held when it went down): this is someone's shortcut, not ours.
    combo: bool,
    swallow_s_up: bool,
    /// Modifiers currently held. Win+Shift+S is the screen snip, Ctrl+Win+...
    /// are desktop shortcuts: with any of these held, we take nothing.
    mods: u8,
}

impl KeyState {
    pub fn on_key(&mut self, vk: u32, down: bool) -> Action {
        let m = modifier_bit(vk);
        if m != 0 {
            if down { self.mods |= m } else { self.mods &= !m }
        }
        let is_win = vk == VK_LWIN || vk == VK_RWIN;
        if is_win && down {
            if !self.win_down {
                self.win_down = true;
                self.combo = self.mods != 0; // fresh press (not auto-repeat)
            }
            Action::Pass
        } else if is_win {
            let was_down = std::mem::take(&mut self.win_down);
            if was_down && !self.combo { Action::WinTap } else { Action::Pass }
        } else if vk == VK_S && down && self.win_down && !self.combo && self.mods == 0 {
            self.combo = true;
            self.swallow_s_up = true;
            Action::WinS
        } else if vk == VK_S && !down && std::mem::take(&mut self.swallow_s_up) {
            Action::Swallow
        } else if vk == VK_ESCAPE && down && self.mods == 2 && !self.win_down {
            Action::RealStart
        } else {
            if down && self.win_down {
                self.combo = true;
            }
            Action::Pass
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    const SHIFT: u32 = 0xA0; // VK_LSHIFT, as the low-level hook reports it
    const CTRL: u32 = 0xA2;
    const E: u32 = 0x45;

    /// Feed (vk, down) events; collect the actions.
    fn run(keys: &[(u32, bool)]) -> Vec<Action> {
        let mut st = KeyState::default();
        keys.iter().map(|&(vk, down)| st.on_key(vk, down)).collect()
    }

    #[test]
    fn lone_win_tap_opens_patina() {
        assert_eq!(run(&[(VK_LWIN, true), (VK_LWIN, false)]), [Pass, WinTap]);
    }

    #[test]
    fn auto_repeat_still_one_tap() {
        assert_eq!(run(&[(VK_LWIN, true), (VK_LWIN, true), (VK_LWIN, true), (VK_LWIN, false)]), [Pass, Pass, Pass, WinTap]);
    }

    #[test]
    fn win_combos_pass_through() {
        assert_eq!(run(&[(VK_LWIN, true), (E, true), (E, false), (VK_LWIN, false)]), [Pass, Pass, Pass, Pass]);
    }

    #[test]
    fn win_s_opens_patina() {
        assert_eq!(run(&[(VK_LWIN, true), (VK_S, true), (VK_S, false), (VK_LWIN, false)]), [Pass, WinS, Swallow, Pass]);
    }

    /// The bug the user hit: Win+Shift+S (screen snip) was taken as Win+S.
    #[test]
    fn win_shift_s_reaches_windows() {
        let seq = [(VK_LWIN, true), (SHIFT, true), (VK_S, true), (VK_S, false), (SHIFT, false), (VK_LWIN, false)];
        assert_eq!(run(&seq), [Pass; 6]);
    }

    #[test]
    fn shift_first_then_win_s_reaches_windows() {
        let seq = [(SHIFT, true), (VK_LWIN, true), (VK_S, true), (VK_S, false), (VK_LWIN, false), (SHIFT, false)];
        assert_eq!(run(&seq), [Pass; 6]);
    }

    #[test]
    fn ctrl_win_s_reaches_windows() {
        let seq = [(CTRL, true), (VK_LWIN, true), (VK_S, true), (VK_S, false), (VK_LWIN, false), (CTRL, false)];
        assert_eq!(run(&seq), [Pass; 6]);
    }

    /// A Win tap with a modifier held is someone's shortcut, not "open Start".
    #[test]
    fn modified_win_tap_is_not_ours() {
        assert_eq!(run(&[(SHIFT, true), (VK_LWIN, true), (VK_LWIN, false), (SHIFT, false)]), [Pass; 4]);
    }

    #[test]
    fn ctrl_esc_is_the_way_to_real_start() {
        const ESC: u32 = 0x1B;
        assert_eq!(run(&[(CTRL, true), (ESC, true), (ESC, false), (CTRL, false)]), [Pass, RealStart, Pass, Pass]);
        // Plain Esc and Ctrl+Shift+Esc (Task Manager) are not it.
        assert_eq!(run(&[(ESC, true), (ESC, false)]), [Pass, Pass]);
        assert_eq!(run(&[(CTRL, true), (SHIFT, true), (ESC, true)]), [Pass, Pass, Pass]);
    }

    #[test]
    fn state_resets_between_presses() {
        let mut seq = vec![(VK_LWIN, true), (SHIFT, true), (VK_S, true), (VK_S, false), (SHIFT, false), (VK_LWIN, false)];
        seq.extend([(VK_LWIN, true), (VK_LWIN, false)]);
        assert_eq!(run(&seq)[6..], [Pass, WinTap]);
    }
}
