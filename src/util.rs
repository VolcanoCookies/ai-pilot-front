use serde::Serialize;

pub fn format_date_relative(date: &chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(*date);

    if duration.num_days() > 0 {
        format!("{} days ago", duration.num_days())
    } else if duration.num_hours() > 0 {
        format!("{} hours ago", duration.num_hours())
    } else if duration.num_minutes() > 0 {
        format!("{} minutes ago", duration.num_minutes())
    } else {
        "just now".to_string()
    }
}

pub fn format_date_time(date: &chrono::DateTime<chrono::Utc>) -> String {
    date.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn discord_avatar_url(discord_id: &str, avatar_hash: &str) -> String {
    format!(
        "https://cdn.discordapp.com/avatars/{}/{}.png",
        discord_id, avatar_hash
    )
}

pub fn format_bytes(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

pub const GIT_COMMIT_HASH: &str = {
    let commit = option_env!("DRONE_COMMIT");
    if let Some(commit) = commit {
        commit
    } else {
        "unknown"
    }
};

const fn get_git_build_date() -> Option<chrono::DateTime<chrono::Utc>> {
    let build_date = option_env!("DRONE_BUILD_STARTED");
    let Some(str) = build_date else {
        return None;
    };
    let Ok(secs) = i64::from_str_radix(str, 10) else {
        return None;
    };
    chrono::DateTime::from_timestamp(secs, 0)
}
pub const GIT_BUILD_DATE: Option<chrono::DateTime<chrono::Utc>> = get_git_build_date();

#[derive(Debug, Clone, Serialize)]
pub struct BuildInfo {
    pub git_hash: &'static str,
    pub build_date: String,
}

pub fn build_info_ctx() -> BuildInfo {
    BuildInfo {
        git_hash: GIT_COMMIT_HASH,
        build_date: GIT_BUILD_DATE
            .as_ref()
            .map_or("unknown".to_string(), format_date_time),
    }
}
