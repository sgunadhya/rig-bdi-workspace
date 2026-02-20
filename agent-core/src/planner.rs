use crate::rules::IncidentPattern;
use crate::runbooks::Runbook;

pub fn select_runbook(
    pattern: IncidentPattern,
    runbooks: &[(&'static str, Runbook)],
) -> Option<(&'static str, Runbook)> {
    let preferred = match pattern {
        IncidentPattern::CrashLoop => "crashloop",
        IncidentPattern::OomKill => "oomkill",
        IncidentPattern::Generic => return None,
    };

    if let Some((name, runbook)) = runbooks
        .iter()
        .find(|(name, _)| name.to_lowercase().contains(preferred))
    {
        return Some((*name, runbook.clone()));
    }

    None
}
