//! CLI #313: named managed coins travel through the offering call, never inputs.
use super::*;

/// The value of named managed coins, used to size a CLI split.
pub async fn from_satoshis<V: WalletServices + Send + Sync + 'static>(
    wallet: &Wallet<StorageSqlx, V>,
    from: &[(String, u32)],
) -> std::result::Result<u64, GateError> {
    let coins = UserCoins::from_wallet(wallet)
        .await
        .map_err(GateError::Gate)?;
    resolve(coins, from)
        .await?
        .iter()
        .try_fold(0u64, |total, (_, value)| {
            total
                .checked_add(*value)
                .ok_or_else(|| GateError::Scope("--from total overflows satoshis".into()))
        })
}

/// Create an action spending every named managed coin, subject to the gate.
/// Empty `from` keeps the ordinary door, including its lazy assessment.
pub async fn create_action_from<V: WalletServices + Send + Sync + 'static>(
    wallet: &Wallet<StorageSqlx, V>,
    mut args: CreateActionArgs,
    originator: &str,
    opts: &GateOptions,
    from: &[(String, u32)],
) -> std::result::Result<CreateActionResult, GateError> {
    if from.is_empty() {
        return create_action(wallet, args, originator, opts).await;
    }
    if args
        .inputs
        .as_ref()
        .is_some_and(|inputs| !inputs.is_empty())
    {
        return Err(GateError::Scope(
            "--from cannot be combined with caller inputs".into(),
        ));
    }
    let coins = UserCoins::from_wallet(wallet)
        .await
        .map_err(GateError::Gate)?;
    let (assessment, ids) = assessment_from(coins, wallet.services(), opts, from).await?;
    strip_rule_label(&mut args.labels);
    if opts.allow_pending.is_on() {
        args.labels.get_or_insert_with(Vec::new).extend([
            RECEIVED_PENDING_LABEL.to_string(),
            TOOLBOX_RECEIVED_LABEL.to_string(),
        ]);
    }
    let outputs = args.outputs.iter().flatten().map(|o| o.satoshis).sum();
    match wallet
        .create_action_offering(args, Some(originator), &assessment.offer(), &ids)
        .await
    {
        Ok(result) => Ok(result),
        Err(
            e @ bsv_sdk::Error::WalletRefusal {
                class: WalletRefusalClass::InsufficientFunds,
                ..
            },
        ) => Err(refusal(coins, &assessment, &e, outputs).await),
        Err(e) => Err(GateError::Wallet(e)),
    }
}

async fn assessment_from<V: WalletServices + ?Sized>(
    coins: UserCoins<'_>,
    services: &V,
    opts: &GateOptions,
    from: &[(String, u32)],
) -> std::result::Result<(Assessment, Vec<i64>), GateError> {
    if let Some(error) = check_scope(coins, opts.allow_pending.named())
        .await
        .map_err(GateError::Gate)?
    {
        return Err(GateError::Scope(error));
    }
    let named = resolve(coins, from).await?;
    let inputs: Vec<_> = from.iter().cloned().map(NamedInput::from).collect();
    let held = assess_named(coins, services, opts, &inputs, &[], None, true).await?;
    reject_held(held)?;
    let mut assessment = assess_inner(coins, services, opts, None, false)
        .await
        .map_err(GateError::Gate)?;
    // Named coins already passed the same gate. Reserve their slots within
    // the offer cap so a small named coin cannot be dropped by value ranking.
    let ids: Vec<_> = named.iter().map(|(id, _)| *id).collect();
    verify_final(&assessment, &ids, from)?;
    let rest = assessment
        .offered
        .iter()
        .copied()
        .filter(|(id, _)| !ids.contains(id))
        .take(OFFER_CAP - named.len())
        .collect::<Vec<_>>();
    assessment.offered = named;
    assessment.offered.extend(rest);
    Ok((assessment, ids))
}

fn reject_held(held: Vec<HeldCoin>) -> std::result::Result<(), GateError> {
    if !held.is_empty() {
        let headers_fault = held
            .iter()
            .all(|coin| coin.headers_fault.is_some())
            .then(|| held.iter().find_map(service_error))
            .flatten();
        return Err(GateError::Refused(Refusal {
            toolbox: NAMED_INPUT_REFUSED.into(),
            offered_satoshis: 0,
            held,
            capped: None,
            headers_fault,
        }));
    }
    Ok(())
}

fn verify_final(
    assessment: &Assessment,
    ids: &[i64],
    from: &[(String, u32)],
) -> std::result::Result<(), GateError> {
    reject_held(
        assessment
            .held
            .iter()
            .filter(|coin| ids.contains(&coin.output_id))
            .cloned()
            .collect(),
    )?;
    for (i, id) in ids.iter().enumerate() {
        if !assessment.offered.iter().any(|(offered, _)| offered == id)
            && !assessment.unread.contains(id)
        {
            return Err(invalid(
                &format!("{}:{}", from[i].0, from[i].1),
                "no longer offered by the final assessment",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_named_coin_lost_between_assessments_is_not_added_back() {
        let mut assessment = Assessment {
            offered: vec![(1, 20)],
            ..Default::default()
        };
        let from = vec![("aa".repeat(32), 0)];
        assert!(matches!(
            verify_final(&assessment, &[2], &from),
            Err(GateError::Scope(_))
        ));
        assessment.unread.push(2);
        assert!(
            verify_final(&assessment, &[2], &from).is_ok(),
            "the cap may omit an otherwise offered coin"
        );
        assessment.held.push(HeldCoin {
            outpoint: format!("{}.0", from[0].0),
            satoshis: 5,
            word: "unknown".into(),
            origin: Origin::Received,
            age_minutes: 1,
            reason: "headers unavailable".into(),
            headers_fault: Some("offline".into()),
            output_id: 2,
        });
        assert!(matches!(
            verify_final(&assessment, &[2], &from),
            Err(GateError::Refused(_))
        ));
    }
}

async fn resolve(
    coins: UserCoins<'_>,
    from: &[(String, u32)],
) -> std::result::Result<Vec<(i64, u64)>, GateError> {
    if from.len() > OFFER_CAP {
        return Err(GateError::Scope(format!(
            "--from names more than {OFFER_CAP} coins"
        )));
    }
    let mut resolved = Vec::with_capacity(from.len());
    for (txid, vout) in from {
        let outpoint = format!("{txid}:{vout}");
        let row = coins
            .outpoint(txid, *vout)
            .await
            .map_err(GateError::Gate)?
            .ok_or_else(|| invalid(&outpoint, NOT_HELD))?;
        let id: i64 = row.get("output_id");
        let why = if resolved.iter().any(|(known, _)| *known == id) {
            Some("named twice")
        } else if row.get::<Option<i64>, _>("spent_by").is_some() {
            Some("spent or reserved")
        } else if row.get::<i64, _>("change") != 1
            || row.get::<Option<String>, _>("provided_by").as_deref() != Some("storage")
            || row.get::<Option<String>, _>("basket").as_deref() != Some("default")
        {
            Some("not wallet-managed change in the default basket")
        } else if !offered_status(&row.get::<String, _>("status")) {
            Some("its transaction is not completed or unproven")
        } else {
            None
        };
        if let Some(why) = why {
            return Err(invalid(&outpoint, why));
        }
        resolved.push((id, row.get::<i64, _>("satoshis").max(0) as u64));
    }
    Ok(resolved)
}

fn invalid(outpoint: &str, why: &str) -> GateError {
    GateError::Scope(named_refusal(
        "--from",
        outpoint,
        "a managed coin this wallet could spend",
        why,
    ))
}
