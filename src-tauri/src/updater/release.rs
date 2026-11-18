//! Selects the newest installable stable release in the running app's major version.
//! GitHub's repository-wide Latest flag is independent of this update channel.

use sai_core::http::client;
use semver::Version;
use serde::Deserialize;
use std::time::Duration;

use super::REPO;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    state: String,
}

impl Release {
    fn has_asset(&self, name: &str) -> bool {
        self.assets
            .iter()
            .any(|asset| asset.name == name && asset.state == "uploaded")
    }
}

fn select_update(current: &Version, releases: &[Release]) -> Option<Version> {
    releases
        .iter()
        .filter_map(|release| {
            if release.draft || release.prerelease {
                return None;
            }
            let version = Version::parse(release.tag_name.strip_prefix('v')?).ok()?;
            if version.major != current.major
                || !version.pre.is_empty()
                || !version.build.is_empty()
                || !version.cmp_precedence(current).is_gt()
                || !release.has_asset("SHA256SUMS.txt")
                || !release.has_asset(&format!("Sai-Setup-{version}.exe"))
            {
                return None;
            }
            Some(version)
        })
        .max()
}

/// Checks up to 100 recent public releases with one request. Returns an unprefixed
/// version only when it is newer than `current`. The caller still verifies the
/// installer against the checksum manifest before treating it as ready.
pub(super) async fn latest_tag(current: &str) -> Result<Option<String>, String> {
    let current =
        Version::parse(current).map_err(|_| "현재 버전을 확인하지 못했어.".to_string())?;
    let response = client()
        .get(format!("https://api.github.com/repos/{REPO}/releases"))
        .query(&[("per_page", 100)])
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("GitHub에 연결하지 못했어: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub에서 새 버전을 확인하지 못했어 (응답 {}).",
            response.status()
        ));
    }
    let releases: Vec<Release> = response
        .json()
        .await
        .map_err(|e| format!("GitHub의 버전 정보를 읽지 못했어: {e}"))?;
    Ok(select_update(&current, &releases).map(|version| version.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            tag_name: format!("v{version}"),
            draft: false,
            prerelease: false,
            assets: [
                "SHA256SUMS.txt".to_string(),
                format!("Sai-Setup-{version}.exe"),
            ]
            .into_iter()
            .map(|name| ReleaseAsset {
                name,
                state: "uploaded".into(),
            })
            .collect(),
        }
    }

    #[test]
    fn selects_highest_version_in_same_major_independent_of_list_order() {
        let current = Version::parse("1.0.1").unwrap();
        let releases = [
            release("0.9.14"),
            release("1.0.10"),
            release("2.0.0"),
            release("1.0.9"),
        ];
        assert_eq!(
            select_update(&current, &releases),
            Some(Version::new(1, 0, 10))
        );
        assert_eq!(select_update(&Version::new(1, 0, 10), &releases), None);
    }

    #[test]
    fn skips_unstable_tags_and_incomplete_assets() {
        let current = Version::new(1, 0, 0);
        let mut draft = release("1.0.8");
        draft.draft = true;
        let mut prerelease = release("1.0.7");
        prerelease.prerelease = true;
        let mut missing_checksum = release("1.0.6");
        missing_checksum.assets.remove(0);
        let mut arm_only = release("1.0.5");
        arm_only.assets[1].name = "Sai-Setup-1.0.5-arm64.exe".into();
        let mut waiting_for_upload = release("1.0.4");
        waiting_for_upload.assets[1].state = "starter".into();
        let mut unprefixed = release("1.0.3");
        unprefixed.tag_name = "1.0.3".into();
        let releases = [
            draft,
            prerelease,
            missing_checksum,
            arm_only,
            waiting_for_upload,
            unprefixed,
            release("1.0.3-rc.1"),
            release("1.0.3+build"),
            release("1.0.2"),
        ];
        assert_eq!(
            select_update(&current, &releases),
            Some(Version::new(1, 0, 2))
        );
    }
}
