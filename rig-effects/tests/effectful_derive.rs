use rig_effects::{Effect, Effectful};
use rig_effects_derive::Effectful;

#[derive(Effectful)]
#[effect(Pure)]
struct ParseJson;

#[derive(Effectful)]
#[effect(Observe)]
struct ReadMetrics;

#[derive(Effectful)]
#[effect(Mutate)]
struct RestartDeployment;

#[derive(Effectful)]
#[effect(Irreversible)]
struct SendPagerDutyAlert;

#[test]
fn derive_supports_all_effect_variants() {
    assert_eq!(ParseJson.effect(), Effect::Pure);
    assert_eq!(ReadMetrics.effect(), Effect::Observe);
    assert_eq!(RestartDeployment.effect(), Effect::Mutate);
    assert_eq!(SendPagerDutyAlert.effect(), Effect::Irreversible);
}
