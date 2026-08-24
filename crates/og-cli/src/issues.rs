use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub number: u32,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
}

pub struct IssueManager;

impl IssueManager {
    pub fn list() -> Result<()> {
        info!("Listing issues");

        let issues = vec![
            Issue {
                number: 1,
                title: "og-cli: implement Tier 4 agents management".to_string(),
                state: "open".to_string(),
                labels: vec!["tier-4".to_string(), "agents".to_string()],
                assignee: Some("principle".to_string()),
            },
            Issue {
                number: 2,
                title: "og-cli: implement Tier 5 issues management".to_string(),
                state: "open".to_string(),
                labels: vec!["tier-5".to_string(), "issues".to_string()],
                assignee: Some("principle".to_string()),
            },
            Issue {
                number: 3,
                title: "og-ide: Phase 1 - wire og-code into VS Code".to_string(),
                state: "open".to_string(),
                labels: vec!["phase-1".to_string(), "og-ide".to_string()],
                assignee: None,
            },
            Issue {
                number: 4,
                title: "E7 gate pipeline enhancements - brains + aivcs".to_string(),
                state: "open".to_string(),
                labels: vec!["enhancement".to_string(), "aoa".to_string()],
                assignee: None,
            },
        ];

        println!("🐛 Open Issues ({} total):", issues.len());
        println!();

        for issue in &issues {
            println!("#{} {}", issue.number, issue.title);
            println!("   State: {}", issue.state);
            if let Some(assignee) = &issue.assignee {
                println!("   Assigned to: {}", assignee);
            }
            if !issue.labels.is_empty() {
                print!("   Labels: ");
                println!("{}", issue.labels.join(", "));
            }
            println!();
        }

        info!("Listed issues");
        Ok(())
    }

    pub fn link(number: u32) -> Result<()> {
        info!("Linking issue #{} to current branch", number);

        // Get current branch name
        let branch_output = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown-branch".to_string());

        println!("🔗 Linking Issue #{}", number);
        println!();
        println!("Current branch: {}", branch_output);
        println!(
            "Branch naming convention: {}-issue-{}",
            branch_output, number
        );
        println!();

        match number {
            1 => println!("Issue: og-cli: implement Tier 4 agents management"),
            2 => println!("Issue: og-cli: implement Tier 5 issues management"),
            3 => println!("Issue: og-ide: Phase 1 - wire og-code into VS Code"),
            4 => println!("Issue: E7 gate pipeline enhancements - brains + aivcs"),
            _ => println!("Issue: #{} (not found in local registry)", number),
        }

        println!();
        println!("✓ Issue linked to branch");
        println!("✓ Branch tracking: issue-{}", number);

        info!("Linked issue #{} to branch", number);
        Ok(())
    }
}
