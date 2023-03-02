use bdk_chain::{bitcoin, miniscript};
use bitcoin::locktime::{Height, Time};
use miniscript::Terminal;

use super::*;

impl<Ak> TermPlan<Ak> {
    fn combine(self, other: Self) -> Option<Self> {
        let min_locktime = {
            match (self.min_locktime, other.min_locktime) {
                (Some(lhs), Some(rhs)) => {
                    if lhs.is_same_unit(rhs) {
                        Some(if lhs.to_consensus_u32() > rhs.to_consensus_u32() {
                            lhs
                        } else {
                            rhs
                        })
                    } else {
                        return None;
                    }
                }
                _ => self.min_locktime.or(other.min_locktime),
            }
        };

        let min_sequence = {
            match (self.min_sequence, other.min_sequence) {
                (Some(lhs), Some(rhs)) => {
                    if lhs.is_height_locked() == rhs.is_height_locked() {
                        Some(if lhs.to_consensus_u32() > rhs.to_consensus_u32() {
                            lhs
                        } else {
                            rhs
                        })
                    } else {
                        return None;
                    }
                }
                _ => self.min_sequence.or(other.min_sequence),
            }
        };

        let mut template = self.template;
        template.extend(other.template);

        Some(Self {
            min_locktime,
            min_sequence,
            template,
        })
    }

    pub(crate) fn expected_size(&self) -> usize {
        self.template.iter().map(|step| step.expected_size()).sum()
    }
}

// impl crate::descriptor::Pkh<DefiniteDescriptorKey> {
//     pub(crate) fn plan_satisfaction<Ak>(&self, assets: &Assets<Ak>) -> Option<Plan<Ak>>
//     where
//         Ak: CanDerive + Clone,
//     {
//         let (asset_key, derivation_hint) = assets.keys.iter().find_map(|asset_key| {
//             let derivation_hint = asset_key.can_derive(self.as_inner())?;
//             Some((asset_key, derivation_hint))
//         })?;

//         Some(Plan {
//             template: vec![TemplateItem::Sign(PlanKey {
//                 asset_key: asset_key.clone(),
//                 descriptor_key: self.as_inner().clone(),
//                 derivation_hint,
//             })],
//             target: Target::Legacy,
//             set_locktime: None,
//             set_sequence: None,
//         })
//     }
// }

// impl crate::descriptor::Wpkh<DefiniteDescriptorKey> {
//     pub(crate) fn plan_satisfaction<Ak>(&self, assets: &Assets<Ak>) -> Option<Plan<Ak>>
//     where
//         Ak: CanDerive + Clone,
//     {
//         let (asset_key, derivation_hint) = assets.keys.iter().find_map(|asset_key| {
//             let derivation_hint = asset_key.can_derive(self.as_inner())?;
//             Some((asset_key, derivation_hint))
//         })?;

//         Some(Plan {
//             template: vec![TemplateItem::Sign(PlanKey {
//                 asset_key: asset_key.clone(),
//                 descriptor_key: self.as_inner().clone(),
//                 derivation_hint,
//             })],
//             target: Target::Segwitv0,
//             set_locktime: None,
//             set_sequence: None,
//         })
//     }
// }

pub(crate) fn plan_satisfaction_tr<Ak>(
    tr: &miniscript::descriptor::Tr<DefiniteDescriptorKey>,
    assets: &Assets<Ak>,
) -> Option<Plan<Ak>>
where
    Ak: CanDerive + Clone,
{
    let key_path_spend = assets.keys.iter().find_map(|asset_key| {
        let derivation_hint = asset_key.can_derive(tr.internal_key())?;
        Some((asset_key, derivation_hint))
    });

    if let Some((asset_key, derivation_hint)) = key_path_spend {
        return Some(Plan {
            template: vec![TemplateItem::Sign(PlanKey {
                asset_key: asset_key.clone(),
                descriptor_key: tr.internal_key().clone(),
                derivation_hint,
            })],
            target: Target::Segwitv1 {
                tr: tr.clone(),
                tr_plan: TrSpend::KeySpend,
            },
            set_locktime: None,
            set_sequence: None,
        });
    }

    let mut plans = tr
        .iter_scripts()
        .filter_map(|(_, ms)| Some((ms, (plan_steps(&ms.node, assets)?))))
        .collect::<Vec<_>>();

    plans.sort_by_cached_key(|(_, plan)| plan.expected_size());

    let (script, best_plan) = plans.into_iter().next()?;

    Some(Plan {
        target: Target::Segwitv1 {
            tr: tr.clone(),
            tr_plan: TrSpend::LeafSpend {
                script: script.encode(),
                leaf_version: LeafVersion::TapScript,
            },
        },
        set_locktime: best_plan.min_locktime.clone(),
        set_sequence: best_plan.min_sequence.clone(),
        template: best_plan.template,
    })
}

#[derive(Debug)]
struct TermPlan<Ak> {
    pub min_locktime: Option<LockTime>,
    pub min_sequence: Option<Sequence>,
    pub template: Vec<TemplateItem<Ak>>,
}

impl<Ak> TermPlan<Ak> {
    fn new(template: Vec<TemplateItem<Ak>>) -> Self {
        TermPlan {
            template,
            ..Default::default()
        }
    }
}

impl<Ak> Default for TermPlan<Ak> {
    fn default() -> Self {
        Self {
            min_locktime: Default::default(),
            min_sequence: Default::default(),
            template: Default::default(),
        }
    }
}

fn plan_steps<Ak: Clone + CanDerive, Ctx: ScriptContext>(
    term: &Terminal<DefiniteDescriptorKey, Ctx>,
    assets: &Assets<Ak>,
) -> Option<TermPlan<Ak>> {
    match term {
        Terminal::True => Some(TermPlan::new(vec![])),
        Terminal::False => return None,
        Terminal::PkH(key) => {
            let (asset_key, derivation_hint) = assets
                .keys
                .iter()
                .find_map(|asset_key| Some((asset_key, asset_key.can_derive(key)?)))?;
            Some(TermPlan::new(vec![
                TemplateItem::Sign(PlanKey {
                    asset_key: asset_key.clone(),
                    derivation_hint,
                    descriptor_key: key.clone(),
                }),
                TemplateItem::Pk { key: key.clone() },
            ]))
        }
        Terminal::PkK(key) => {
            let (asset_key, derivation_hint) = assets
                .keys
                .iter()
                .find_map(|asset_key| Some((asset_key, asset_key.can_derive(key)?)))?;
            Some(TermPlan::new(vec![TemplateItem::Sign(PlanKey {
                asset_key: asset_key.clone(),
                derivation_hint,
                descriptor_key: key.clone(),
            })]))
        }
        Terminal::RawPkH(_pk_hash) => {
            /* TODO */
            None
        }
        Terminal::After(locktime) => {
            let max_locktime = assets.max_locktime?;
            let locktime = LockTime::from(locktime);
            let (height, time) = match max_locktime {
                LockTime::Blocks(height) => (height, Time::from_consensus(0).unwrap()),
                LockTime::Seconds(seconds) => (Height::from_consensus(0).unwrap(), seconds),
            };
            if max_locktime.is_satisfied_by(height, time) {
                Some(TermPlan {
                    min_locktime: Some(locktime),
                    ..Default::default()
                })
            } else {
                None
            }
        }
        Terminal::Older(older) => {
            // FIXME: older should be a height or time not a sequence.
            let max_sequence = assets.txo_age?;
            //TODO: this whole thing is probably wrong but upstream should provide a way of
            // doing it properly.
            if max_sequence.is_height_locked() == older.is_height_locked() {
                if max_sequence.to_consensus_u32() >= older.to_consensus_u32() {
                    Some(TermPlan {
                        min_sequence: Some(*older),
                        ..Default::default()
                    })
                } else {
                    None
                }
            } else {
                None
            }
        }
        Terminal::Sha256(image) => {
            if assets.sha256.contains(&image) {
                Some(TermPlan::new(vec![TemplateItem::Sha256(image.clone())]))
            } else {
                None
            }
        }
        Terminal::Hash256(image) => {
            if assets.hash256.contains(image) {
                Some(TermPlan::new(vec![TemplateItem::Hash256(image.clone())]))
            } else {
                None
            }
        }
        Terminal::Ripemd160(image) => {
            if assets.ripemd160.contains(&image) {
                Some(TermPlan::new(vec![TemplateItem::Ripemd160(image.clone())]))
            } else {
                None
            }
        }
        Terminal::Hash160(image) => {
            if assets.hash160.contains(&image) {
                Some(TermPlan::new(vec![TemplateItem::Hash160(image.clone())]))
            } else {
                None
            }
        }
        Terminal::Alt(ms)
        | Terminal::Swap(ms)
        | Terminal::Check(ms)
        | Terminal::Verify(ms)
        | Terminal::NonZero(ms)
        | Terminal::ZeroNotEqual(ms) => plan_steps(&ms.node, assets),
        Terminal::DupIf(ms) => {
            let mut plan = plan_steps(&ms.node, assets)?;
            plan.template.push(TemplateItem::One);
            Some(plan)
        }
        Terminal::AndV(l, r) | Terminal::AndB(l, r) => {
            let lhs = plan_steps(&l.node, assets)?;
            let rhs = plan_steps(&r.node, assets)?;
            lhs.combine(rhs)
        }
        Terminal::AndOr(_, _, _) => todo!(),
        Terminal::OrB(_, _) => todo!(),
        Terminal::OrD(_, _) => todo!(),
        Terminal::OrC(_, _) => todo!(),
        Terminal::OrI(lhs, rhs) => {
            let lplan = plan_steps(&lhs.node, assets).map(|mut plan| {
                plan.template.push(TemplateItem::One);
                plan
            });
            let rplan = plan_steps(&rhs.node, assets).map(|mut plan| {
                plan.template.push(TemplateItem::Zero);
                plan
            });
            match (lplan, rplan) {
                (Some(lplan), Some(rplan)) => {
                    if lplan.expected_size() <= rplan.expected_size() {
                        Some(lplan)
                    } else {
                        Some(rplan)
                    }
                }
                (lplan, rplan) => lplan.or(rplan),
            }
        }
        Terminal::Thresh(_, _) => todo!(),
        Terminal::Multi(_, _) => todo!(),
        Terminal::MultiA(_, _) => todo!(),
    }
}
