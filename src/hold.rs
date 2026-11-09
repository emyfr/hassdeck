//! Tasti protetti da pressioni accidentali: l'azione parte solo dopo
//! `HOLD_DURATION` di pressione continua, mentre il tasto e' ancora premuto.
//! Un rilascio anticipato annulla la pressione.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

pub const HOLD_DURATION: Duration = Duration::from_secs(2);

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

    /// Registra il rilascio: `Some(true)` se l'azione era gia' partita,
    /// `Some(false)` se il tasto e' stato rilasciato troppo presto, `None` se
    /// la pressione non era di un tasto protetto.
    pub async fn release(&self, key: u8) -> Option<bool> {
        self.presses.lock().await.remove(&key).map(|press| press.fired)
    }
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
    async fn unprotected_release_is_not_tracked() {
        let holds = HoldTracker::default();
        assert_eq!(holds.release(7).await, None);
    }
}
