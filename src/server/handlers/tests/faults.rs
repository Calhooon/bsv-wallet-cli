//! The typed faults, the abort record and the unjudged input of bsv-rs
//! 0.9.0-beta.4 (#439, #452) as the host map answers them (#342).

use super::*;

/// The error as each door hands it to `AppError`, with the Display each
/// serves: the SDK's own, the toolbox's wrapper (its `SDK error: ` prefix)
/// and a handler's `anyhow`.
fn every_door(error: &bsv_sdk::Error) -> [(AppError, String); 3] {
    let toolbox = bsv_wallet_toolbox::Error::SdkError(error.clone());
    let toolbox_display = toolbox.to_string();
    [
        (AppError::from(error.clone()), error.to_string()),
        (AppError::from(toolbox), toolbox_display),
        (
            AppError::from(anyhow::Error::new(error.clone())),
            error.to_string(),
        ),
    ]
}

/// A toolbox fault is typed by its family since beta.4 (bsv-rs #439, #342):
/// the far side's (transport, service) is 502 `SERVICE_ERROR`, the wallet's
/// own (storage, internal) 500 `INTERNAL_ERROR`, whatever its words; the
/// message is Display's, the same as the untyped `WalletError`'s.
#[tokio::test]
async fn a_typed_wallet_fault_is_answered_by_its_family() {
    use bsv_sdk::WalletFaultFamily as F;
    for (family, status, code) in [
        (F::Transport, 502, "SERVICE_ERROR"),
        (F::Service, 502, "SERVICE_ERROR"),
        (F::Storage, 500, "INTERNAL_ERROR"),
        (F::Internal, 500, "INTERNAL_ERROR"),
    ] {
        for message in ["Insufficient funds: need 5", "Store busy: sqlite code 5"] {
            let error = bsv_sdk::Error::WalletFault(bsv_sdk::WalletFault::new(family, message));
            let display = error.to_string();
            assert_eq!(
                display,
                bsv_sdk::Error::WalletError(message.into()).to_string()
            );
            for (mapped, display) in every_door(&error) {
                let (actual, retry, body) = answer(mapped).await;
                assert_eq!(actual.as_u16(), status, "{family:?}");
                assert_eq!(body, json!({"code": code, "message": display}));
                assert!(retry.is_none());
            }
        }
    }
}

/// An abort refused on stored acceptance is the typed record (#342): its
/// status is its class's, as a `WalletRefusal` of that class, and the
/// recovery hint is selected on the record and names its known fields;
/// the same words as a plain refusal carry no hint.
#[tokio::test]
async fn an_accepted_abort_keeps_its_class_and_carries_the_records_hint() {
    use bsv_sdk::WalletRefusalClass as C;
    let message = "Invalid transaction status: cannot abort action: transaction aa has a \
                   recorded acceptance (tracker word 'seen'); only a transaction never accepted \
                   can be retired";
    for (class, status, code) in [
        (C::InvalidParameter, 400, "VALIDATION_ERROR"),
        (C::InvalidOperation, 409, "CONFLICT"),
    ] {
        let mut record = bsv_sdk::AbortAcceptance::new("ref-aa", class, message);
        record.txid = Some("aa".into());
        record.status = Some("unproven".into());
        record.accepting_word = Some("tracker word 'seen'".into());
        let error = bsv_sdk::Error::AbortAccepted(Box::new(record));
        assert_eq!(
            error.to_string(),
            bsv_sdk::Error::WalletRefusal {
                class,
                message: message.into()
            }
            .to_string()
        );
        let hint = "(hint: the wallet holds an accepting word for this transaction \
                    (transaction aa, status 'unproven', accepting word tracker word 'seen'); it \
                    stays until a proof, node verdict or competitor's checked proof decides it)";
        for (mapped, display) in every_door(&error) {
            let (actual, _, body) = answer(mapped).await;
            assert_eq!(actual.as_u16(), status, "{class:?}");
            assert_eq!(body["code"], code);
            assert_eq!(body["message"], format!("{display} {hint}"));
        }
        let plain = bsv_sdk::Error::WalletRefusal {
            class,
            message: message.into(),
        };
        let (_, _, body) = answer(AppError::from(plain)).await;
        assert_eq!(body["message"], message);
    }
    let bare = bsv_sdk::AbortAcceptance::new("ref", C::InvalidParameter, "m");
    let (_, _, body) = answer(AppError::from(bsv_sdk::Error::AbortAccepted(Box::new(
        bare,
    ))))
    .await;
    assert_eq!(
        body["message"],
        "m (hint: the wallet holds an accepting word for this transaction; it stays until a \
         proof, node verdict or competitor's checked proof decides it)"
    );
}

/// An input the stack could not judge (bsv-rs #452) is answered by its
/// cause: a coin or spend before Genesis is the wallet's state (409), the
/// stack's tip, activation heights, source height or work budget (per input
/// or, since bsv-rs 0.9.0-beta.5's #519, per transaction) its own fault
/// (500); never the caller's 400.
#[tokio::test]
async fn a_not_judged_input_is_answered_by_its_cause() {
    use bsv_sdk::script::NotJudgedCause as C;
    for (cause, status, code) in [
        (
            C::PreGenesisCoin {
                height: 620_537,
                genesis_height: 620_538,
            },
            409,
            "CONFLICT",
        ),
        (
            C::PreGenesisSpend {
                height: 901,
                genesis_height: 620_538,
            },
            409,
            "CONFLICT",
        ),
        (
            C::TipHeightOverflow { tip: u32::MAX },
            500,
            "INTERNAL_ERROR",
        ),
        (
            C::SourceHeightOverflow { height: u64::MAX },
            500,
            "INTERNAL_ERROR",
        ),
        (
            C::WorkBudgetExceeded {
                input: 0,
                budget: 1 << 31,
                work_done: 1 << 31,
            },
            500,
            "INTERNAL_ERROR",
        ),
        (
            C::TransactionWorkBudgetExceeded {
                input: 1,
                limit: 1 << 31,
                total: (1 << 31) + 1,
            },
            500,
            "INTERNAL_ERROR",
        ),
    ] {
        let error = bsv_sdk::Error::TransactionNotJudged {
            txid: "aa".repeat(32),
            input_index: 0,
            cause,
        };
        let display = error.to_string();
        let (actual, _, body) = answer(AppError::from(error)).await;
        assert_eq!(actual.as_u16(), status, "{cause:?}");
        assert_eq!(body, json!({"code": code, "message": display}));
    }
}
