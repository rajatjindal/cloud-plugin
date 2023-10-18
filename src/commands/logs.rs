use std::ops::Sub;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::Utc;
use cloud_openapi::models::Entry;

use cloud::{CloudClientExt, CloudClientInterface};

use crate::commands::create_cloud_client;
use crate::commands::deploy::{DEVELOPER_CLOUD_FAQ, SPIN_DEPLOY_CHANNEL_NAME};
use crate::opts::*;
use clap::Parser;
use uuid::Uuid;

/// Fetch and tail logs of an application deployed to the Fermyon Cloud.
#[derive(Parser, Debug)]
#[clap(about = "fetch logs for an app from Fermyon Cloud")]
pub struct LogsCommand {
    /// Find app to fetch logs for in the Fermyon instance saved under the specified name.
    /// If omitted, Spin looks for app in default unnamed instance.
    #[clap(
        name = "environment-name",
        long = "environment-name",
        env = DEPLOYMENT_ENV_NAME_ENV
    )]
    pub deployment_env_id: Option<String>,

    /// App name
    pub app: String,

    /// Follow logs output
    #[clap(name = "follow", long = "follow")]
    pub follow: bool,

    /// Number of lines to show from the end of the logs
    #[clap(name = "tail", long = "tail", default_value = "10")]
    pub max_lines: i32,

    /// Interval in seconds to refresh logs from cloud
    #[clap(parse(try_from_str = parse_interval), name="interval", long="interval", default_value = "2")]
    pub interval_secs: u64,

    /// Only return logs newer than a relative duration. The duration format is a number
    /// and a unit, where the unit is 's' for seconds, 'm' for minutes, 'h' for hours
    /// or 'd' for days (e.g. "30m" for 30 minutes ago).  The default it 7 days.
    #[clap(parse(try_from_str = parse_duration), name="since", long="since", default_value = "7d")]
    pub since: std::time::Duration,
}

impl LogsCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.deployment_env_id.as_deref()).await?;
        self.logs(&client)
            .await
            .map_err(|e| anyhow!("{:?}\n\nLearn more at {}", e, DEVELOPER_CLOUD_FAQ))
    }

    async fn logs(self, client: &impl CloudClientInterface) -> Result<()> {
        let app_id = match client
            .get_app_id(&self.app)
            .await
            .map_err(|e| anyhow!("app with name {:?} not found\n\n{:?}", &self.app, e))?
        {
            Some(x) => x,
            None => return Err(anyhow!("app with name {:?} not found", &self.app)),
        };

        let channel_id = client
            .get_channel_id(app_id, SPIN_DEPLOY_CHANNEL_NAME)
            .await
            .map_err(|e| {
                anyhow!(
                    "logs channel for app with name {:?} not found\n\n{:?}",
                    &self.app,
                    e
                )
            })?;

        fetch_logs_and_print_loop(
            client,
            channel_id,
            self.follow,
            self.interval_secs,
            Some(self.max_lines),
            self.since,
        )
        .await?;

        Ok(())
    }
}

async fn fetch_logs_and_print_loop(
    client: &impl CloudClientInterface,
    channel_id: Uuid,
    follow: bool,
    interval: u64,
    max_lines: Option<i32>,
    since: Duration,
) -> Result<()> {
    let mut new_since = Utc::now().sub(since).to_rfc3339();
    new_since =
        fetch_logs_and_print_once(client, channel_id, max_lines, new_since.to_owned()).await?;

    if !follow {
        return Ok(());
    }

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
        new_since =
            fetch_logs_and_print_once(client, channel_id, None, new_since.to_owned()).await?;
    }
}

async fn fetch_logs_and_print_once(
    client: &impl CloudClientInterface,
    channel_id: Uuid,
    max_lines: Option<i32>,
    since: String,
) -> Result<String> {
    let entries = client
        .channel_logs_raw(channel_id.to_string(), max_lines, Some(since.to_string()))
        .await?
        .entries;

    if entries.is_empty() {
        return Ok(since.to_owned());
    }

    Ok(print_lastn_logs(&entries).to_owned())
}

fn print_lastn_logs(entries: &[Entry]) -> &str {
    let mut new_since: &str = "";
    for entry in entries.iter().rev() {
        for line in entry.log_lines.as_ref().unwrap() {
            // line will always have some content, so it is safe to unwrap
            println!("{}", line.line.as_ref().unwrap());
            new_since = line.time.as_ref().unwrap().as_str()
        }
    }

    new_since
}

fn parse_duration(arg: &str) -> anyhow::Result<std::time::Duration> {
    let duration = if let Some(parg) = arg.strip_suffix('s') {
        let value = parg.parse()?;
        std::time::Duration::from_secs(value)
    } else if let Some(parg) = arg.strip_suffix('m') {
        let value: u64 = parg.parse()?;
        std::time::Duration::from_secs(value * 60)
    } else if let Some(parg) = arg.strip_suffix('h') {
        let value: u64 = parg.parse()?;
        std::time::Duration::from_secs(value * 60 * 60)
    } else if let Some(parg) = arg.strip_suffix('d') {
        let value: u64 = parg.parse()?;
        std::time::Duration::from_secs(value * 24 * 60 * 60)
    } else {
        return Err(anyhow!(
            r#"supported formats are "300s", "5m", "4h" or "1d". formats such as "1h30m" or "30min" are not supported"#
        ));
    };

    Ok(duration)
}

fn parse_interval(arg: &str) -> anyhow::Result<u64> {
    let interval = arg.parse()?;
    if interval < 2 {
        return Err(anyhow!("interval cannot be less than 2 seconds"));
    }

    Ok(interval)
}
