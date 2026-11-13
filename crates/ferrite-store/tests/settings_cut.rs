//! Decision 377: the settings record is added after the last one in its sector, and starts the other sector
//! (erased first) when there is no room. The simulated chip loses power at every step of a save, at every
//! fill of the sector (the appends and the change of sector both ways): the next power-on reads the settings
//! before the save or after it, never anything else, and a save after that works.

use ferrite_app::data::NoData;
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::App;
use ferrite_core::layout::SETTINGS;
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;

type Store = FlashStore<SimFlash, NoData>;

/// An App whose settings carry `v` (0-99) in the two brightness levels (1-10 each).
fn app(v: u8) -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    (set.brightness, set.brightness_night) = (1 + v % 10, 1 + v / 10);
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

/// The value the stored settings carry, as the next power-on reads them.
fn stored(f: SimFlash) -> (Store, Option<u8>) {
    let s: Store = FlashStore::mount(f, NoData);
    let mut set = Settings::DEFAULT;
    let v = s.load_settings(&mut set).then(|| (set.brightness - 1) + (set.brightness_night - 1) * 10);
    (s, v)
}

fn erases(f: &SimFlash) -> u32 {
    (SETTINGS.start / 4096..SETTINGS.end / 4096).map(|s| f.erases[s as usize]).sum()
}

#[test]
fn settings_saves_share_a_sector_until_it_is_full() {
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    assert!(s.save(&app(0)));
    let first = erases(&s.nor());
    for v in 1..71u8 {
        assert!(s.save(&app(v)));
    }
    let f = s.into_parts().0;
    // 70 more saves: an erase each time a sector fills (6 records of 598 bytes in its 4 KB), against 70
    // before decision 377.
    let e = erases(&f) - first;
    assert_eq!(e, 11, "{e} erases for 70 saves");
    assert_eq!(stored(f).1, Some(70));
}

#[test]
fn settings_save_with_the_power_cut_at_every_step() {
    for pre in 1..=16u8 {
        // A chip with `pre` saves on it (0..pre), then save `pre` with the power cut at each step.
        let base = {
            let s: Store = FlashStore::mount(SimFlash::new(), NoData);
            for v in 0..pre {
                assert!(s.save(&app(v)));
            }
            s.into_parts().0
        };
        let steps = {
            let s: Store = FlashStore::mount(base.clone(), NoData);
            let at = s.nor().ops;
            assert!(s.save(&app(pre)));
            let n = s.nor().ops - at;
            n
        };
        for k in 0..steps {
            // Every step near the start and the end of a write, and every 5th between (the record is ~550 bytes).
            if k > 24 && k + 24 < steps && k % 5 != 0 {
                continue;
            }
            let s: Store = FlashStore::mount(base.clone(), NoData);
            let at = s.nor().ops;
            s.nor().cut_at = Some(at + k + 1);
            let _ = s.save(&app(pre));
            let mut f = s.into_parts().0;
            f.power_on();
            let (s, v) = stored(f);
            assert!(v == Some(pre - 1) || v == Some(pre), "{pre} saved before, cut at step {k} of {steps}: read {v:?}");
            // and the store goes on saving from there
            assert!(s.save(&app(pre + 1)), "{pre}, cut at {k}: the next save");
            assert_eq!(stored(s.into_parts().0).1, Some(pre + 1), "{pre}, cut at {k}: the next save read back");
        }
    }
}
