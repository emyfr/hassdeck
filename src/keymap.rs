//! Mappatura tra la numerazione fisica dei tasti del Soomfon XF-CN001 (1-15,
//! da sinistra a destra, dall'alto in basso) e gli indici logici usati dal
//! protocollo del dispositivo (canale vendor `mirajazz`), determinata
//! empiricamente — vedi ANALYSIS.md.
//!
//! Il firmware assegna gli indici per colonna partendo da destra, dall'alto
//! in basso. L'indice riportato in lettura (pressione tasto) è sempre
//! l'indice di scrittura + 1.

/// Indici logici di scrittura delle tre zone della barra verticale,
/// dall'alto in basso.
pub const BAR_WRITE_INDICES: [u8; 3] = [15, 16, 17];

/// Indice logico (0-17) da usare per scrivere l'icona del tasto fisico `physical_key` (1-15).
pub fn write_index_for_physical_key(physical_key: u8) -> u8 {
    let idx0 = physical_key - 1;
    let row = idx0 / 5;
    let col_in_row = idx0 % 5;
    let group = 4 - col_in_row;
    group * 3 + row
}

/// Tasto fisico (1-15) corrispondente all'indice riportato in lettura, se
/// corrisponde a un tasto reale. Restituisce `None` per gli indici della
/// barra verticale o non riconosciuti.
pub fn physical_key_for_read_index(read_index: u8) -> Option<u8> {
    if read_index == 0 {
        return None;
    }
    let write_index = read_index - 1;
    if write_index > 14 {
        return None;
    }
    let group = write_index / 3;
    let row = write_index % 3;
    let col_in_row = 4 - group;
    Some(row * 5 + col_in_row + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_index_matches_confirmed_table() {
        let expected = [
            (1, 12),
            (2, 9),
            (3, 6),
            (4, 3),
            (5, 0),
            (6, 13),
            (7, 10),
            (8, 7),
            (9, 4),
            (10, 1),
            (11, 14),
            (12, 11),
            (13, 8),
            (14, 5),
            (15, 2),
        ];

        for (key, index) in expected {
            assert_eq!(write_index_for_physical_key(key), index, "tasto {key}");
        }
    }

    #[test]
    fn read_index_is_write_index_plus_one() {
        for key in 1u8..=15 {
            let write_index = write_index_for_physical_key(key);
            assert_eq!(physical_key_for_read_index(write_index + 1), Some(key));
        }
    }

    #[test]
    fn bar_indices_are_not_real_keys() {
        assert_eq!(physical_key_for_read_index(0), None);
        assert_eq!(physical_key_for_read_index(16), None);
        assert_eq!(physical_key_for_read_index(17), None);
        assert_eq!(physical_key_for_read_index(18), None);
    }
}
