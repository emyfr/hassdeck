//! Pressioni prolungate dei tasti.
//!
//! - Tasti protetti da pressioni accidentali: l'azione parte solo dopo
//!   `HOLD_DURATION` di pressione continua, mentre il tasto e' ancora premuto.
//!   Un rilascio anticipato annulla la pressione.
//! - Tasti luce: un rilascio prima di `LIGHT_LONG_PRESS` accende o spegne la
//!   luce; oltre, la luminosita' cambia ogni `LIGHT_STEP_INTERVAL` fino al
//!   rilascio, salendo e scendendo a pressioni alterne (vedi `light_direction`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

pub const HOLD_DURATION: Duration = Duration::from_secs(2);
pub const LIGHT_LONG_PRESS: Duration = Duration::from_millis(500);
pub const LIGHT_STEP_INTERVAL: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy)]
struct Press {
    id: u64,
    fired: bool,
}

/// Pressioni in corso sui tasti protetti. Ogni pressione ha un id, cosi' un
/// timer rimasto da una pressione precedente non puo' far partire l'azione
/// di quella successiva.
#[derive(Clone, Default)]
pub struct HoldTracker {
    presses: Arc<Mutex<HashMap<u8, Press>>>,
    next_id: Arc<Mutex<u64>>,
}

impl HoldTracker {
    /// Registra l'inizio di una pressione e ne restituisce l'id.
    pub async fn press(&self, key: u8) -> u64 {
        let id = {
            let mut next = self.next_id.lock().await;
            *next += 1;
            *next
        };
        self.presses.lock().await.insert(key, Press { id, fired: false });
        id
    }

    /// Chiamata allo scadere di `HOLD_DURATION`: restituisce `true` (una sola
    /// volta) se la pressione `id` e' ancora in corso, e la segna come eseguita.
    pub async fn fire_if_held(&self, key: u8, id: u64) -> bool {
        let mut presses = self.presses.lock().await;
        match presses.get_mut(&key) {
            Some(press) if press.id == id && !press.fired => {
                press.fired = true;
                true
            }
            _ => false,
        }
    }

    /// `true` finche' la pressione `id` non e' stata rilasciata.
    pub async fn is_held(&self, key: u8, id: u64) -> bool {
        self.presses.lock().await.get(&key).is_some_and(|press| press.id == id)
    }

    /// Registra il rilascio: `Some(true)` se l'azione era gia' partita,
    /// `Some(false)` se il tasto e' stato rilasciato troppo presto, `None` se
    /// la pressione non era seguita da questo tracker.
    pub async fn release(&self, key: u8) -> Option<bool> {
        self.presses.lock().await.remove(&key).map(|press| press.fired)
    }
}

/// Verso dell'ultima regolazione di luminosita' per ogni tasto luce (`true`
/// se saliva), per alternare salita e discesa tra una pressione e l'altra.
#[derive(Clone, Default)]
pub struct LightDirections {
    last_up: Arc<Mutex<HashMap<u8, bool>>>,
}

impl LightDirections {
    /// Sceglie il verso della nuova regolazione in base alla luminosita'
    /// attuale (0-255, `None` se la luce e' spenta) e lo ricorda.
    pub async fn next(&self, key: u8, brightness: Option<u8>) -> bool {
        let mut last_up = self.last_up.lock().await;
        let up = light_direction(brightness, last_up.get(&key).copied());
        last_up.insert(key, up);
        up
    }
}

/// Verso della regolazione: sale se la luce e' spenta o al minimo, scende se
/// e' al massimo, altrimenti inverte la regolazione precedente (la prima sale).
pub fn light_direction(brightness: Option<u8>, last_up: Option<bool>) -> bool {
    match brightness {
        None => true,
        Some(255) => false,
        Some(b) if brightness_pct(b) <= 1 => true,
        Some(_) => last_up.map_or(true, |up| !up),
    }
}

/// Passi da `step_pct` necessari per portare la luminosita' dal valore
/// attuale (0-255, `None` se spenta) al 100% salendo o allo 0% scendendo.
pub fn light_steps(brightness: Option<u8>, up: bool, step_pct: u8) -> u32 {
    let pct = brightness.map_or(0, brightness_pct);
    let remaining = if up { 100 - pct } else { pct };
    remaining.div_ceil(u32::from(step_pct.max(1))).max(1)
}

/// Luminosita' 0-255 di Home Assistant espressa in percentuale.
pub fn brightness_pct(brightness: u8) -> u32 {
    (u32::from(brightness) * 100 + 127) / 255
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fires_once_while_held() {
        let holds = HoldTracker::default();
        let id = holds.press(3).await;
        assert!(holds.fire_if_held(3, id).await);
        assert!(!holds.fire_if_held(3, id).await);
        assert_eq!(holds.release(3).await, Some(true));
    }

    #[tokio::test]
    async fn early_release_cancels() {
        let holds = HoldTracker::default();
        let id = holds.press(3).await;
        assert_eq!(holds.release(3).await, Some(false));
        assert!(!holds.fire_if_held(3, id).await);
    }

    #[tokio::test]
    async fn stale_timer_does_not_fire_next_press() {
        let holds = HoldTracker::default();
        let first = holds.press(3).await;
        holds.release(3).await;
        let second = holds.press(3).await;
        assert!(!holds.fire_if_held(3, first).await);
        assert!(holds.fire_if_held(3, second).await);
    }

    #[tokio::test]
    async fn held_until_release() {
        let holds = HoldTracker::default();
        let first = holds.press(3).await;
        assert!(holds.is_held(3, first).await);
        holds.release(3).await;
        assert!(!holds.is_held(3, first).await);
        let second = holds.press(3).await;
        assert!(!holds.is_held(3, first).await);
        assert!(holds.is_held(3, second).await);
    }

    #[test]
    fn light_direction_alternates() {
        assert!(light_direction(Some(128), None));
        assert!(!light_direction(Some(128), Some(true)));
        assert!(light_direction(Some(128), Some(false)));
    }

    #[test]
    fn light_direction_at_the_ends() {
        assert!(light_direction(None, Some(true)));
        assert!(light_direction(Some(2), Some(true)));
        assert!(!light_direction(Some(255), Some(false)));
    }

    #[test]
    fn light_steps_to_the_end() {
        assert_eq!(light_steps(None, true, 10), 10);
        assert_eq!(light_steps(Some(255), false, 10), 10);
        assert_eq!(light_steps(Some(128), true, 10), 5);
        assert_eq!(light_steps(Some(128), false, 30), 2);
        assert_eq!(light_steps(Some(255), true, 10), 1);
    }

    #[tokio::test]
    async fn light_directions_remembered_per_key() {
        let dirs = LightDirections::default();
        assert!(dirs.next(11, Some(128)).await);
        assert!(!dirs.next(11, Some(200)).await);
        assert!(dirs.next(12, Some(128)).await);
        assert!(dirs.next(11, Some(100)).await);
    }

    #[tokio::test]
    async fn unprotected_release_is_not_tracked() {
        let holds = HoldTracker::default();
        assert_eq!(holds.release(7).await, None);
    }
}
