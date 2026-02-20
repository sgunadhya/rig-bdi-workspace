use crate::facts::{AlertFact, Fact};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IncidentPattern {
    CrashLoop,
    OomKill,
    Generic,
}

pub fn detect_pattern(fact: &Fact) -> IncidentPattern {
    match fact {
        Fact::Alert(AlertFact { title, tags, .. }) => {
            let title_lower = title.to_lowercase();
            let tags_lower = tags
                .iter()
                .map(|t| t.to_lowercase())
                .collect::<Vec<_>>();

            if title_lower.contains("crashloop")
                || tags_lower.iter().any(|t| t.contains("crashloop"))
            {
                IncidentPattern::CrashLoop
            } else if title_lower.contains("oom")
                || title_lower.contains("out of memory")
                || tags_lower.iter().any(|t| t.contains("oom"))
            {
                IncidentPattern::OomKill
            } else {
                IncidentPattern::Generic
            }
        }
    }
}
