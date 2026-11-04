//! The level-0 charge, once per call (#556): a door's level-0 powers read
//! the bound in the call's order and the call moves it once, when every
//! power has admitted it. Driven through [`Asker::ask`], the path every
//! handler's powers take, with the bound's clock stopped so no window
//! refills between the calls.

use axum::response::IntoResponse;

use super::*;
use crate::server::owner_door_tests::wallet;

const DOOR: &str = "/createSignature";

/// A page's door view over `wallet`, its bound's clock stopped.
fn asker(wallet: &WalletState) -> Asker {
    Asker {
        wallet: wallet.clone(),
        held: Held::default(),
        probes: Probes::default().started(wallet),
        bounds: OpenBounds::stopped(),
        admitted: Some(Admitted {
            origin: "http://page.example".into(),
            originator: "page.example".into(),
            seed: None,
        }),
    }
}

fn level_zero(name: &str) -> Ask {
    Ask::protocol(0, name, None, "signing")
}

/// One call asking `asks`: its answer's status (`200` for `Ok`).
async fn call(asker: &Asker, asks: Vec<Ask>) -> u16 {
    match asker.ask(DOOR, None, asks).await {
        Ok(()) => 200,
        Err(e) => e.into_response().status().as_u16(),
    }
}

/// Nine single level-0 calls: one of the burst's ten left.
async fn nine_spent(asker: &Asker) {
    for n in 0..9 {
        assert_eq!(call(asker, vec![level_zero("open gate")]).await, 200, "{n}");
    }
}

/// The level-0 answers counted in the probe record for the page.
async fn counted(asker: &Asker) -> i64 {
    asker.probes.settled().await;
    let all = grants::listing(asker.wallet.storage(), true).await.unwrap();
    all.probes
        .iter()
        .filter(|r| r.originator == "page.example")
        .map(|r| r.count)
        .sum()
}

/// A refusal at a later check leaves the level-0 counter where it stood:
/// with one call of the burst left, a call whose level-0 power admits and
/// whose next power (an admin-reserved basket) refuses is answered 400,
/// and the burst's last call is still admitted after it; the refused call
/// is not counted.
#[tokio::test]
async fn a_later_refusal_leaves_the_level_zero_counter_where_it_stood() {
    let (_dir, wallet) = wallet().await;
    let asker = asker(&wallet);
    nine_spent(&asker).await;
    let refused = vec![level_zero("open gate"), Ask::basket("default", "listing")];
    assert_eq!(call(&asker, refused).await, 400);
    assert_eq!(
        call(&asker, vec![level_zero("open gate")]).await,
        200,
        "the burst's tenth call is whole"
    );
    assert_eq!(call(&asker, vec![level_zero("open gate")]).await, 429);
    assert_eq!(counted(&asker).await, 10, "the refused call is not counted");
}

/// A call that passes two level-0 checks is charged once: with one call of
/// the burst left, a call asking two level-0 powers is admitted (charged
/// at its first check, its second was refused 429), and the next call is
/// past the burst. Each of its powers is counted.
#[tokio::test]
async fn a_call_passing_two_level_zero_checks_is_charged_once() {
    let (_dir, wallet) = wallet().await;
    let asker = asker(&wallet);
    nine_spent(&asker).await;
    let both = vec![level_zero("open gate"), level_zero("second gate")];
    assert_eq!(
        call(&asker, both).await,
        200,
        "charged once, within the burst"
    );
    assert_eq!(call(&asker, vec![level_zero("open gate")]).await, 429);
    assert_eq!(
        counted(&asker).await,
        11,
        "nine, then the call's two powers"
    );
}

/// Past the bound, a level-0 check still refuses in its place: the 429 at
/// once, before a later power is asked (an admin-reserved basket would be
/// 400), and nothing counted.
#[tokio::test]
async fn past_the_bound_the_level_zero_check_refuses_in_its_place() {
    let (_dir, wallet) = wallet().await;
    let asker = asker(&wallet);
    nine_spent(&asker).await;
    assert_eq!(call(&asker, vec![level_zero("open gate")]).await, 200);
    let past = vec![level_zero("open gate"), Ask::basket("default", "listing")];
    assert_eq!(call(&asker, past).await, 429);
    assert_eq!(counted(&asker).await, 10);
}
