// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Application state and logic.

use chrono::{DateTime, Utc};
use runtara_management_sdk::{
    Checkpoint, CheckpointSummary, GetTenantMetricsOptions, HealthStatus, ImageSummary,
    InstanceInfo, InstanceStatus, InstanceSummary, ListCheckpointsOptions, ListImagesOptions,
    ListInstancesOptions, ManagementSdk, MetricsGranularity, SdkConfig, TenantMetricsResult,
};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// Status filter for instances list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Running,
    Completed,
    Failed,
    Pending,
    Suspended,
}

impl StatusFilter {
    pub fn as_str(&self) -> &'static str {
        match self {
            StatusFilter::All => "All",
            StatusFilter::Running => "Running",
            StatusFilter::Completed => "Completed",
            StatusFilter::Failed => "Failed",
            StatusFilter::Pending => "Pending",
            StatusFilter::Suspended => "Suspended",
        }
    }

    pub fn to_instance_status(&self) -> Option<InstanceStatus> {
        match self {
            StatusFilter::All => None,
            StatusFilter::Running => Some(InstanceStatus::Running),
            StatusFilter::Completed => Some(InstanceStatus::Completed),
            StatusFilter::Failed => Some(InstanceStatus::Failed),
            StatusFilter::Pending => Some(InstanceStatus::Pending),
            StatusFilter::Suspended => Some(InstanceStatus::Suspended),
        }
    }

    pub fn next(&self) -> Self {
        match self {
            StatusFilter::All => StatusFilter::Running,
            StatusFilter::Running => StatusFilter::Completed,
            StatusFilter::Completed => StatusFilter::Failed,
            StatusFilter::Failed => StatusFilter::Pending,
            StatusFilter::Pending => StatusFilter::Suspended,
            StatusFilter::Suspended => StatusFilter::All,
        }
    }
}

/// Active tab in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Instances,
    Images,
    Metrics,
    Health,
}

impl Tab {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tab::Instances => "Instances",
            Tab::Images => "Images",
            Tab::Metrics => "Metrics",
            Tab::Health => "Health",
        }
    }

    pub fn all() -> &'static [Tab] {
        &[Tab::Instances, Tab::Images, Tab::Metrics, Tab::Health]
    }
}

/// Current view mode (main list or detail view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Main list view
    #[default]
    List,
    /// Instance detail view (modal)
    InstanceDetail,
    /// Checkpoints list for an instance
    CheckpointsList,
    /// Checkpoint detail view (JSON data)
    CheckpointDetail,
}

/// Application state.
pub struct App {
    /// SDK configuration
    pub server_addr: SocketAddr,
    skip_cert_verification: bool,

    /// Optional tenant filter
    pub tenant_id: Option<String>,

    /// Current tab
    pub tab: Tab,

    /// Current view mode
    pub view_mode: ViewMode,

    /// Status filter for instances
    pub status_filter: StatusFilter,

    /// Health status
    pub health: Option<HealthStatus>,

    /// List of instances
    pub instances: Vec<InstanceSummary>,
    pub instances_total: u32,
    pub instances_selected: usize,

    /// List of images
    pub images: Vec<ImageSummary>,
    pub images_total: u32,
    pub images_selected: usize,

    /// Instance detail view
    pub instance_detail: Option<InstanceInfo>,

    /// Checkpoints list for current instance
    pub checkpoints: Vec<CheckpointSummary>,
    pub checkpoints_total: u32,
    pub checkpoints_selected: usize,

    /// Checkpoint detail view
    pub checkpoint_detail: Option<Checkpoint>,

    /// Metrics data
    pub metrics: Option<TenantMetricsResult>,
    pub metrics_granularity: MetricsGranularity,
    pub metrics_selected: usize,

    /// Scroll offset for detail views
    pub detail_scroll: u16,

    /// Last refresh time
    pub last_refresh: Option<Instant>,
    pub refresh_interval: Duration,

    /// Error message (if any)
    pub error: Option<String>,

    /// Connection status
    pub connected: bool,
}

impl App {
    pub fn new(
        server: &str,
        skip_cert_verification: bool,
        tenant_id: Option<String>,
        refresh_interval: Duration,
    ) -> Self {
        let server_addr: SocketAddr = server
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:8002".parse().unwrap());

        Self {
            server_addr,
            skip_cert_verification,
            tenant_id,
            tab: Tab::Instances,
            view_mode: ViewMode::List,
            status_filter: StatusFilter::All,
            health: None,
            instances: Vec::new(),
            instances_total: 0,
            instances_selected: 0,
            images: Vec::new(),
            images_total: 0,
            images_selected: 0,
            instance_detail: None,
            checkpoints: Vec::new(),
            checkpoints_total: 0,
            checkpoints_selected: 0,
            checkpoint_detail: None,
            metrics: None,
            metrics_granularity: MetricsGranularity::Hourly,
            metrics_selected: 0,
            detail_scroll: 0,
            last_refresh: None,
            refresh_interval,
            error: None,
            connected: false,
        }
    }

    /// Create SDK instance
    fn create_sdk(&self) -> Result<ManagementSdk, runtara_management_sdk::SdkError> {
        let config = SdkConfig {
            server_addr: self.server_addr,
            server_name: "localhost".to_string(),
            skip_cert_verification: self.skip_cert_verification,
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(10),
        };
        ManagementSdk::new(config)
    }

    /// Refresh all data from server
    pub async fn refresh(&mut self) {
        self.error = None;

        let sdk = match self.create_sdk() {
            Ok(sdk) => sdk,
            Err(e) => {
                self.error = Some(format!("Failed to create SDK: {}", e));
                self.connected = false;
                return;
            }
        };

        if let Err(e) = sdk.connect().await {
            self.error = Some(format!("Connection failed: {}", e));
            self.connected = false;
            return;
        }

        self.connected = true;

        // Fetch health
        match sdk.health_check().await {
            Ok(health) => self.health = Some(health),
            Err(e) => {
                self.error = Some(format!("Health check failed: {}", e));
            }
        }

        // Fetch instances
        let options = ListInstancesOptions {
            tenant_id: self.tenant_id.clone(),
            status: self.status_filter.to_instance_status(),
            limit: 100,
            ..Default::default()
        };

        match sdk.list_instances(options).await {
            Ok(result) => {
                self.instances = result.instances;
                self.instances_total = result.total_count;
                if self.instances_selected >= self.instances.len() && !self.instances.is_empty() {
                    self.instances_selected = self.instances.len() - 1;
                }
            }
            Err(e) => {
                self.error = Some(format!("Failed to list instances: {}", e));
            }
        }

        // Fetch images
        let options = ListImagesOptions {
            tenant_id: self.tenant_id.clone(),
            limit: 100,
            ..Default::default()
        };

        match sdk.list_images(options).await {
            Ok(result) => {
                self.images = result.images;
                self.images_total = result.total_count;
                if self.images_selected >= self.images.len() && !self.images.is_empty() {
                    self.images_selected = self.images.len() - 1;
                }
            }
            Err(e) => {
                self.error = Some(format!("Failed to list images: {}", e));
            }
        }

        // Fetch metrics (requires tenant_id)
        if let Some(ref tenant_id) = self.tenant_id {
            let options =
                GetTenantMetricsOptions::new(tenant_id).with_granularity(self.metrics_granularity);

            match sdk.get_tenant_metrics(options).await {
                Ok(result) => {
                    let bucket_count = result.buckets.len();
                    self.metrics = Some(result);
                    if self.metrics_selected >= bucket_count && bucket_count > 0 {
                        self.metrics_selected = bucket_count - 1;
                    }
                }
                Err(e) => {
                    self.error = Some(format!("Failed to get metrics: {}", e));
                }
            }
        }

        self.last_refresh = Some(Instant::now());
    }

    /// Check if we should auto-refresh
    pub fn should_refresh(&self) -> bool {
        match self.last_refresh {
            Some(last) => last.elapsed() >= self.refresh_interval,
            None => true,
        }
    }

    /// Switch to next tab
    pub fn next_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Instances => Tab::Images,
            Tab::Images => Tab::Metrics,
            Tab::Metrics => Tab::Health,
            Tab::Health => Tab::Instances,
        };
    }

    /// Switch to previous tab
    pub fn previous_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Instances => Tab::Health,
            Tab::Images => Tab::Instances,
            Tab::Metrics => Tab::Images,
            Tab::Health => Tab::Metrics,
        };
    }

    /// Set tab by index
    pub fn set_tab(&mut self, index: usize) {
        self.tab = match index {
            0 => Tab::Instances,
            1 => Tab::Images,
            2 => Tab::Metrics,
            3 => Tab::Health,
            _ => Tab::Instances,
        };
    }

    /// Select next item in current list
    pub fn next_item(&mut self) {
        match self.tab {
            Tab::Instances => {
                if !self.instances.is_empty() {
                    self.instances_selected = (self.instances_selected + 1) % self.instances.len();
                }
            }
            Tab::Images => {
                if !self.images.is_empty() {
                    self.images_selected = (self.images_selected + 1) % self.images.len();
                }
            }
            Tab::Metrics => {
                if let Some(ref metrics) = self.metrics {
                    if !metrics.buckets.is_empty() {
                        self.metrics_selected = (self.metrics_selected + 1) % metrics.buckets.len();
                    }
                }
            }
            Tab::Health => {}
        }
    }

    /// Select previous item in current list
    pub fn previous_item(&mut self) {
        match self.tab {
            Tab::Instances => {
                if !self.instances.is_empty() {
                    self.instances_selected = self
                        .instances_selected
                        .checked_sub(1)
                        .unwrap_or(self.instances.len() - 1);
                }
            }
            Tab::Images => {
                if !self.images.is_empty() {
                    self.images_selected = self
                        .images_selected
                        .checked_sub(1)
                        .unwrap_or(self.images.len() - 1);
                }
            }
            Tab::Metrics => {
                if let Some(ref metrics) = self.metrics {
                    if !metrics.buckets.is_empty() {
                        self.metrics_selected = self
                            .metrics_selected
                            .checked_sub(1)
                            .unwrap_or(metrics.buckets.len() - 1);
                    }
                }
            }
            Tab::Health => {}
        }
    }

    /// Cycle through status filters
    pub fn cycle_status_filter(&mut self) {
        self.status_filter = self.status_filter.next();
    }

    /// Toggle metrics granularity between hourly and daily
    pub fn toggle_metrics_granularity(&mut self) {
        self.metrics_granularity = match self.metrics_granularity {
            MetricsGranularity::Hourly => MetricsGranularity::Daily,
            MetricsGranularity::Daily => MetricsGranularity::Hourly,
        };
        self.metrics_selected = 0;
    }

    /// Open instance detail view for the selected instance
    pub async fn open_instance_detail(&mut self) {
        if self.instances.is_empty() {
            return;
        }

        let instance_id = &self.instances[self.instances_selected].instance_id;

        let sdk = match self.create_sdk() {
            Ok(sdk) => sdk,
            Err(e) => {
                self.error = Some(format!("Failed to create SDK: {}", e));
                return;
            }
        };

        if let Err(e) = sdk.connect().await {
            self.error = Some(format!("Connection failed: {}", e));
            return;
        }

        match sdk.get_instance_status(instance_id).await {
            Ok(info) => {
                self.instance_detail = Some(info);
                self.view_mode = ViewMode::InstanceDetail;
                self.detail_scroll = 0;
            }
            Err(e) => {
                self.error = Some(format!("Failed to get instance details: {}", e));
            }
        }
    }

    /// Open checkpoints list for the current instance detail
    pub async fn open_checkpoints_list(&mut self) {
        let instance_id = match &self.instance_detail {
            Some(info) => info.instance_id.clone(),
            None => return,
        };

        let sdk = match self.create_sdk() {
            Ok(sdk) => sdk,
            Err(e) => {
                self.error = Some(format!("Failed to create SDK: {}", e));
                return;
            }
        };

        if let Err(e) = sdk.connect().await {
            self.error = Some(format!("Connection failed: {}", e));
            return;
        }

        let options = ListCheckpointsOptions::new().with_limit(100);

        match sdk.list_checkpoints(&instance_id, options).await {
            Ok(result) => {
                self.checkpoints = result.checkpoints;
                self.checkpoints_total = result.total_count;
                self.checkpoints_selected = 0;
                self.view_mode = ViewMode::CheckpointsList;
            }
            Err(e) => {
                self.error = Some(format!("Failed to list checkpoints: {}", e));
            }
        }
    }

    /// Open checkpoint detail view for the selected checkpoint
    pub async fn open_checkpoint_detail(&mut self) {
        if self.checkpoints.is_empty() {
            return;
        }

        let checkpoint = &self.checkpoints[self.checkpoints_selected];
        let instance_id = checkpoint.instance_id.clone();
        let checkpoint_id = checkpoint.checkpoint_id.clone();

        let sdk = match self.create_sdk() {
            Ok(sdk) => sdk,
            Err(e) => {
                self.error = Some(format!("Failed to create SDK: {}", e));
                return;
            }
        };

        if let Err(e) = sdk.connect().await {
            self.error = Some(format!("Connection failed: {}", e));
            return;
        }

        match sdk.get_checkpoint(&instance_id, &checkpoint_id).await {
            Ok(Some(checkpoint)) => {
                self.checkpoint_detail = Some(checkpoint);
                self.view_mode = ViewMode::CheckpointDetail;
                self.detail_scroll = 0;
            }
            Ok(None) => {
                self.error = Some("Checkpoint not found".to_string());
            }
            Err(e) => {
                self.error = Some(format!("Failed to get checkpoint: {}", e));
            }
        }
    }

    /// Go back to previous view
    pub fn go_back(&mut self) {
        match self.view_mode {
            ViewMode::List => {
                // Already at top level, do nothing
            }
            ViewMode::InstanceDetail => {
                self.view_mode = ViewMode::List;
                self.instance_detail = None;
                self.detail_scroll = 0;
            }
            ViewMode::CheckpointsList => {
                self.view_mode = ViewMode::InstanceDetail;
                self.checkpoints.clear();
                self.checkpoints_total = 0;
                self.checkpoints_selected = 0;
            }
            ViewMode::CheckpointDetail => {
                self.view_mode = ViewMode::CheckpointsList;
                self.checkpoint_detail = None;
                self.detail_scroll = 0;
            }
        }
    }

    /// Scroll detail view up
    pub fn scroll_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
    }

    /// Scroll detail view down
    pub fn scroll_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(1);
    }

    /// Navigate in checkpoints list
    pub fn next_checkpoint(&mut self) {
        if !self.checkpoints.is_empty() {
            self.checkpoints_selected = (self.checkpoints_selected + 1) % self.checkpoints.len();
        }
    }

    /// Navigate in checkpoints list
    pub fn previous_checkpoint(&mut self) {
        if !self.checkpoints.is_empty() {
            self.checkpoints_selected = self
                .checkpoints_selected
                .checked_sub(1)
                .unwrap_or(self.checkpoints.len() - 1);
        }
    }
}

/// Format a datetime for display
pub fn format_datetime(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Format a duration for display
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;

    if days > 0 {
        format!("{}d {}h", days, hours % 24)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins % 60)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Format instance status with color hint
pub fn status_style(status: InstanceStatus) -> (&'static str, ratatui::style::Color) {
    use ratatui::style::Color;
    match status {
        InstanceStatus::Pending => ("Pending", Color::Yellow),
        InstanceStatus::Running => ("Running", Color::Blue),
        InstanceStatus::Suspended => ("Suspended", Color::Magenta),
        InstanceStatus::Completed => ("Completed", Color::Green),
        InstanceStatus::Failed => ("Failed", Color::Red),
        InstanceStatus::Cancelled => ("Cancelled", Color::Gray),
        InstanceStatus::Unknown => ("Unknown", Color::DarkGray),
    }
}
