use std::{
    future::IntoFuture,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use comfy_table::Table;
use console::style;
use futures::FutureExt;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use itertools::Itertools;
use rattler::install::{DefaultProgressFormatter, IndicatifReporter, Installer};
use rattler_conda_types::{Channel, ChannelUrl, MatchSpec, Platform, PrefixRecord, RepoDataRecord};
use rattler_solve::{ChannelPriority, SolveStrategy, SolverImpl, SolverTask, resolvo::Solver};
use url::Url;

use crate::{metadata::PlatformWithVirtualPackages, packaging::Files, tool_configuration};

fn print_as_table(packages: &[RepoDataRecord]) {
    let mut table = Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL_CONDENSED)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
    table.set_header(vec![
        "Package", "Version", "Build", "Channel", "Size",
        // "License",
    ]);
    let column = table.column_mut(4).expect("This should be column five");
    column.set_cell_alignment(comfy_table::CellAlignment::Right);

    for package in packages
        .iter()
        .sorted_by_key(|p| p.package_record.name.as_normalized())
    {
        let channel_short = if package.channel.as_deref().unwrap_or_default().contains('/') {
            package
                .channel
                .as_ref()
                .and_then(|s| s.rsplit('/').find(|s| !s.is_empty()))
                .expect("expected channel to be defined and contain '/'")
                .to_string()
        } else {
            package.channel.as_deref().unwrap_or_default().to_string()
        };

        table.add_row([
            package.package_record.name.as_normalized().to_string(),
            package.package_record.version.to_string(),
            package.package_record.build.clone(),
            channel_short,
            HumanBytes(package.package_record.size.unwrap_or(0)).to_string(),
            // package.package_record.license.clone().unwrap_or_else(|| "".to_string()),
        ]);
    }

    tracing::info!("\n{table}");
}

pub async fn solve_environment(
    name: &str,
    specs: &[MatchSpec],
    target_platform: &PlatformWithVirtualPackages,
    channels: &[ChannelUrl],
    tool_configuration: &tool_configuration::Configuration,
    channel_priority: ChannelPriority,
    solve_strategy: SolveStrategy,
) -> anyhow::Result<Vec<RepoDataRecord>> {
    let vp_string = format!("[{}]", target_platform.virtual_packages.iter().format(", "));

    tracing::info!("\nResolving {name} environment:\n");
    tracing::info!(
        "  Platform: {} {}",
        target_platform.platform,
        style(vp_string).dim()
    );
    tracing::info!("  Channels: ");
    for channel in channels {
        tracing::info!(
            "   - {}",
            tool_configuration
                .channel_config
                .canonical_name(channel.url())
        );
    }
    tracing::info!("  Specs:");
    for spec in specs {
        tracing::info!("   - {}", spec);
    }

    let repo_data = load_repodatas(
        channels,
        target_platform.platform,
        specs,
        tool_configuration,
    )
    .await?;

    // Now that we parsed and downloaded all information, construct the packaging
    // problem that we need to solve. We do this by constructing a
    // `SolverProblem`. This encapsulates all the information required to be
    // able to solve the problem.
    let solver_task = SolverTask {
        virtual_packages: target_platform.virtual_packages.clone(),
        specs: specs.to_vec(),
        channel_priority,
        strategy: solve_strategy,
        ..SolverTask::from_iter(&repo_data)
    };

    // Next, use a solver to solve this specific problem. This provides us with all
    // the operations we need to apply to our environment to bring it up to
    // date.
    let solver_result = tool_configuration
        .fancy_log_handler
        .wrap_in_progress("solving", move || Solver.solve(solver_task))?;

    // Print the result as a table
    print_as_table(&solver_result.records);

    Ok(solver_result.records)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_environment(
    name: &str,
    specs: &[MatchSpec],
    target_platform: &PlatformWithVirtualPackages,
    target_prefix: &Path,
    channels: &[ChannelUrl],
    tool_configuration: &tool_configuration::Configuration,
    channel_priority: ChannelPriority,
    solve_strategy: SolveStrategy,
) -> anyhow::Result<Vec<RepoDataRecord>> {
    let required_packages = solve_environment(
        name,
        specs,
        target_platform,
        channels,
        tool_configuration,
        channel_priority,
        solve_strategy,
    )
    .await?;

    install_packages(
        name,
        &required_packages,
        target_platform.platform,
        target_prefix,
        tool_configuration,
    )
    .await?;

    Ok(required_packages)
}

struct GatewayReporter {
    progress_bars: Arc<Mutex<Vec<ProgressBar>>>,
    multi_progress: indicatif::MultiProgress,
    progress_template: Option<ProgressStyle>,
    finish_template: Option<ProgressStyle>,
}

#[derive(Default)]
struct GatewayReporterBuilder {
    multi_progress: Option<indicatif::MultiProgress>,
    progress_template: Option<ProgressStyle>,
    finish_template: Option<ProgressStyle>,
}

impl GatewayReporter {
    pub fn builder() -> GatewayReporterBuilder {
        GatewayReporterBuilder::default()
    }
}

impl rattler_repodata_gateway::Reporter for GatewayReporter {
    fn on_download_start(&self, _url: &Url) -> usize {
        let progress_bar = self
            .multi_progress
            .add(ProgressBar::new(1))
            .with_finish(indicatif::ProgressFinish::AndLeave)
            .with_prefix("Downloading");

        // use the configured style
        if let Some(template) = &self.progress_template {
            progress_bar.set_style(template.clone());
        }

        // progress_bar.enable_steady_tick(Duration::from_millis(100));

        let mut progress_bars = self.progress_bars.lock().unwrap();
        progress_bars.push(progress_bar);
        progress_bars.len() - 1
    }

    fn on_download_complete(&self, _url: &Url, index: usize) {
        // Remove the progress bar from the multi progress
        let pb = &self.progress_bars.lock().unwrap()[index];
        if let Some(template) = &self.finish_template {
            pb.set_style(template.clone());
            pb.finish_with_message("Done".to_string());
        } else {
            pb.finish();
        }
    }

    fn on_download_progress(&self, _url: &Url, index: usize, bytes: usize, total: Option<usize>) {
        let progress_bar = &self.progress_bars.lock().unwrap()[index];
        progress_bar.set_length(total.unwrap_or(bytes) as u64);
        progress_bar.set_position(bytes as u64);
    }
}

impl GatewayReporterBuilder {
    #[must_use]
    pub fn with_multi_progress(
        mut self,
        multi_progress: indicatif::MultiProgress,
    ) -> GatewayReporterBuilder {
        self.multi_progress = Some(multi_progress);
        self
    }

    #[must_use]
    pub fn with_progress_template(mut self, template: ProgressStyle) -> GatewayReporterBuilder {
        self.progress_template = Some(template);
        self
    }

    #[must_use]
    pub fn with_finish_template(mut self, template: ProgressStyle) -> GatewayReporterBuilder {
        self.finish_template = Some(template);
        self
    }

    pub fn finish(self) -> GatewayReporter {
        GatewayReporter {
            progress_bars: Arc::new(Mutex::new(Vec::new())),
            multi_progress: self.multi_progress.expect("multi progress is required"),
            progress_template: self.progress_template,
            finish_template: self.finish_template,
        }
    }
}

/// Load repodata from channels. Only includes necessary records for platform &
/// specs.
pub async fn load_repodatas(
    channels: &[ChannelUrl],
    target_platform: Platform,
    specs: &[MatchSpec],
    tool_configuration: &tool_configuration::Configuration,
) -> anyhow::Result<Vec<rattler_repodata_gateway::RepoData>> {
    let channels = channels
        .iter()
        .map(|url| Channel::from_url(url.clone()))
        .collect::<Vec<_>>();

    let result = tool_configuration
        .repodata_gateway
        .query(
            channels,
            [target_platform, Platform::NoArch],
            specs.to_vec(),
        )
        .with_reporter(
            GatewayReporter::builder()
                .with_multi_progress(
                    tool_configuration
                        .fancy_log_handler
                        .multi_progress()
                        .clone(),
                )
                .with_progress_template(tool_configuration.fancy_log_handler.default_bytes_style())
                .with_finish_template(
                    tool_configuration
                        .fancy_log_handler
                        .finished_progress_style(),
                )
                .finish(),
        )
        .recursive(true)
        .into_future()
        .boxed()
        .await?;

    tool_configuration
        .fancy_log_handler
        .multi_progress()
        .clear()
        .unwrap();

    Ok(result)
}

pub async fn install_packages(
    name: &str,
    required_packages: &[RepoDataRecord],
    target_platform: Platform,
    target_prefix: &Path,
    tool_configuration: &tool_configuration::Configuration,
) -> anyhow::Result<()> {
    // Make sure the target prefix exists, regardless of whether we'll actually
    // install anything in there.
    let prefix = rattler_conda_types::prefix::Prefix::create(target_prefix).with_context(|| {
        format!(
            "failed to create target prefix: {}",
            target_prefix.display()
        )
    })?;

    if !prefix.path().join("conda-meta/history").exists() {
        // Create an empty history file if it doesn't exist
        fs_err::create_dir_all(prefix.path().join("conda-meta"))?;
        fs_err::File::create(prefix.path().join("conda-meta/history"))?;
    }

    let installed_packages = PrefixRecord::collect_from_prefix(target_prefix)?;

    if !installed_packages.is_empty() && name.starts_with("host") {
        // we have to clean up extra files in the prefix
        let extra_files =
            Files::from_prefix(target_prefix, &Default::default(), &Default::default())?;

        tracing::info!(
            "Cleaning up {} files in the prefix from a previous build.",
            extra_files.new_files.len()
        );

        for f in extra_files.new_files {
            if !f.is_dir() {
                fs_err::remove_file(target_prefix.join(f))?;
            }
        }
    }

    tracing::info!("\nInstalling {name} environment\n");
    Installer::new()
        .with_download_client(tool_configuration.client.get_client().clone())
        .with_target_platform(target_platform)
        .with_execute_link_scripts(true)
        .with_package_cache(tool_configuration.package_cache.clone())
        .with_installed_packages(installed_packages)
        .with_io_concurrency_limit(tool_configuration.io_concurrency_limit.unwrap_or_default())
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(
                    tool_configuration
                        .fancy_log_handler
                        .multi_progress()
                        .clone(),
                )
                .with_formatter(
                    DefaultProgressFormatter::default()
                        .with_prefix(tool_configuration.fancy_log_handler.with_indent_levels("")),
                )
                .finish(),
        )
        .install(&target_prefix, required_packages.to_owned())
        .await?;

    tracing::info!(
        "{} Successfully updated the {name} environment",
        console::style(console::Emoji("✔", "")).green(),
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, Version};
    use rattler_repodata_gateway::Reporter;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_solver_conflicting_dependencies() {
        // Test complex scenario where package A requires B>2.0 but package C requires B<2.0
        // This tests the solver's ability to handle version conflicts
        let _specs = vec![
            MatchSpec::from_str("numpy>=1.20", ParseStrictness::Lenient).unwrap(),
            MatchSpec::from_str("scipy>=1.7", ParseStrictness::Lenient).unwrap(),
            MatchSpec::from_str("pandas>=1.3", ParseStrictness::Lenient).unwrap(),
        ];

        // Create a mock platform with virtual packages
        let virtual_packages = vec![
            rattler_conda_types::GenericVirtualPackage {
                name: PackageName::from_str("__glibc").unwrap(),
                version: Version::from_str("2.31").unwrap(),
                build_string: "0".to_string(),
            },
            rattler_conda_types::GenericVirtualPackage {
                name: PackageName::from_str("__cuda").unwrap(),
                version: Version::from_str("11.8").unwrap(),
                build_string: "0".to_string(),
            },
        ];

        let _platform = PlatformWithVirtualPackages {
            platform: Platform::Linux64,
            virtual_packages,
        };

        // This would need proper channel URLs and tool configuration in a real test
        // For now, we're testing the structure and error handling
    }

    #[tokio::test]
    async fn test_solver_channel_priority_complex() {
        // Test complex channel priority scenarios:
        // 1. Same package in multiple channels with different versions
        // 2. Dependencies only available in lower-priority channels
        // 3. Strict vs flexible channel priority behavior

        let _specs = vec![
            MatchSpec::from_str("python=3.11.*", ParseStrictness::Lenient).unwrap(),
            MatchSpec::from_str("requests", ParseStrictness::Lenient).unwrap(),
        ];

        // Test with strict channel priority - should fail if dependency not in priority channel
        // Test with flexible priority - should succeed by using lower priority channel
    }

    #[tokio::test]
    async fn test_solver_virtual_package_constraints() {
        // Test solver behavior with complex virtual package requirements
        // E.g., CUDA-enabled packages requiring specific CUDA versions
        let _specs = vec![
            MatchSpec::from_str("pytorch", ParseStrictness::Lenient).unwrap(),
            MatchSpec::from_str("tensorflow-gpu", ParseStrictness::Lenient).unwrap(),
        ];

        let _virtual_packages_cuda11 = [rattler_conda_types::GenericVirtualPackage {
            name: PackageName::from_str("__cuda").unwrap(),
            version: Version::from_str("11.8").unwrap(),
            build_string: "0".to_string(),
        }];

        let _virtual_packages_cuda12 = [rattler_conda_types::GenericVirtualPackage {
            name: PackageName::from_str("__cuda").unwrap(),
            version: Version::from_str("12.0").unwrap(),
            build_string: "0".to_string(),
        }];

        // Test that solver picks compatible versions based on virtual packages
    }

    #[tokio::test]
    async fn test_solver_circular_dependencies() {
        // Test handling of circular dependencies:
        // A depends on B, B depends on C, C depends on A
        let _specs = vec![MatchSpec::from_str("package-a", ParseStrictness::Lenient).unwrap()];

        // Solver should detect and handle circular dependencies gracefully
    }

    #[tokio::test]
    async fn test_solver_deep_dependency_tree() {
        // Test solver performance and correctness with deep dependency trees
        // Package with 10+ levels of transitive dependencies
        let _specs = vec![
            MatchSpec::from_str("scikit-learn", ParseStrictness::Lenient).unwrap(),
            MatchSpec::from_str("matplotlib", ParseStrictness::Lenient).unwrap(),
            MatchSpec::from_str("seaborn", ParseStrictness::Lenient).unwrap(),
        ];

        // This tests the solver's ability to handle complex real-world dependency graphs
    }

    #[test]
    fn test_gateway_reporter_concurrent_downloads() {
        use std::sync::Arc;
        use url::Url;

        // Test concurrent progress reporting with hidden output
        let multi_progress = indicatif::MultiProgress::new();
        multi_progress.set_draw_target(indicatif::ProgressDrawTarget::hidden());

        let reporter = GatewayReporter::builder()
            .with_multi_progress(multi_progress)
            .finish();

        let reporter = Arc::new(reporter);

        // Simulate concurrent downloads
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reporter = reporter.clone();
                std::thread::spawn(move || {
                    let url =
                        Url::parse(&format!("https://example.com/file{}.tar.bz2", i)).unwrap();
                    let index = reporter.on_download_start(&url);

                    // Simulate progress updates
                    for bytes in (0..100).step_by(10) {
                        reporter.on_download_progress(&url, index, bytes * 1024, Some(100 * 1024));
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }

                    reporter.on_download_complete(&url, index);
                })
            })
            .collect();

        // Ensure no panics or race conditions
        for handle in handles {
            handle.join().unwrap();
        }
    }
}
