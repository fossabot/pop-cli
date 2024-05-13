// SPDX-License-Identifier: GPL-3.0
use crate::errors::Error;
use anyhow::Result;
use git2::{
	build::RepoBuilder, FetchOptions, IndexAddOption, RemoteCallbacks, Repository, ResetType,
};
use git2_credentials::CredentialHandler;
use regex::Regex;
use std::path::Path;
use std::{env, fs};
use url::Url;

pub struct Git;
impl Git {
	pub(crate) fn clone(url: &Url, working_dir: &Path, branch: Option<&str>) -> Result<()> {
		if !working_dir.exists() {
			let mut fo = FetchOptions::new();
			fo.depth(1);
			let mut repo = RepoBuilder::new();
			repo.fetch_options(fo);
			if let Some(branch) = branch {
				repo.branch(branch);
			}
			if let Err(_e) = repo.clone(url.as_str(), working_dir) {
				Self::ssh_clone(url, working_dir, branch)?;
			}
		}
		Ok(())
	}
	pub(crate) fn ssh_clone(url: &Url, working_dir: &Path, branch: Option<&str>) -> Result<()> {
		let ssh_url = GitHub::convert_to_ssh_url(url);
		if !working_dir.exists() {
			// Prepare callback and fetch options.
			let mut fo = FetchOptions::new();
			Self::set_up_ssh_fetch_options(&mut fo)?;
			// Prepare builder and clone.
			let mut repo = RepoBuilder::new();
			repo.fetch_options(fo);
			if let Some(branch) = branch {
				repo.branch(branch);
			}
			repo.clone(&ssh_url, working_dir)?;
		}
		Ok(())
	}
	/// Clone `url` into `target` and degit it
	pub fn clone_and_degit(
		url: &str,
		target: &Path,
		tag_version: Option<String>,
	) -> Result<Option<String>> {
		let repo = match Repository::clone(url, target) {
			Ok(repo) => repo,
			Err(_e) => Self::ssh_clone_and_degit(
				url::Url::parse(url).map_err(|err| Error::from(err))?,
				target,
			)?,
		};

		if let Some(tag_version) = tag_version {
			let (object, reference) = repo.revparse_ext(&tag_version).expect("Object not found");
			repo.checkout_tree(&object, None).expect("Failed to checkout");
			match reference {
				// gref is an actual reference like branches or tags
				Some(gref) => repo.set_head(gref.name().unwrap()),
				// this is a commit, not a reference
				None => repo.set_head_detached(object.id()),
			}
			.expect("Failed to set HEAD");

			let git_dir = repo.path();
			fs::remove_dir_all(&git_dir)?;
			return Ok(Some(tag_version));
		}

		// fetch tags from remote
		let release = Self::fetch_latest_tag(&repo);

		let git_dir = repo.path();
		fs::remove_dir_all(&git_dir)?;
		// Or by default the last one
		Ok(release)
	}

	/// For users that have ssh configuration for cloning repositories
	fn ssh_clone_and_degit(url: Url, target: &Path) -> Result<Repository> {
		let ssh_url = GitHub::convert_to_ssh_url(&url);
		// Prepare callback and fetch options.
		let mut fo = FetchOptions::new();
		Self::set_up_ssh_fetch_options(&mut fo)?;
		// Prepare builder and clone.
		let mut builder = RepoBuilder::new();
		builder.fetch_options(fo);
		let repo = builder.clone(&ssh_url, target)?;
		Ok(repo)
	}

	fn set_up_ssh_fetch_options(fo: &mut FetchOptions) -> Result<()> {
		let mut callbacks = RemoteCallbacks::new();
		let git_config = git2::Config::open_default()
			.map_err(|e| Error::Config(format!("Cannot open git configuration: {}", e)))?;
		let mut ch = CredentialHandler::new(git_config);
		callbacks.credentials(move |url, username, allowed| {
			ch.try_next_credential(url, username, allowed)
		});

		fo.remote_callbacks(callbacks);
		Ok(())
	}

	/// Fetch the latest release from a repository
	fn fetch_latest_tag(repo: &Repository) -> Option<String> {
		let version_reg = Regex::new(r"v\d+\.\d+\.\d+").expect("Valid regex");
		let tags = repo.tag_names(None).ok()?;
		// Start from latest tags
		for tag in tags.iter().rev() {
			if let Some(tag) = tag {
				if version_reg.is_match(tag) {
					return Some(tag.to_string());
				}
			}
		}
		None
	}

	/// Init a new git repo on creation of a parachain
	pub fn git_init(target: &Path, message: &str) -> Result<(), git2::Error> {
		let repo = Repository::init(target)?;
		let signature = repo.signature()?;

		let mut index = repo.index()?;
		index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
		let tree_id = index.write_tree()?;

		let tree = repo.find_tree(tree_id)?;
		let commit_id = repo.commit(Some("HEAD"), &signature, &signature, message, &tree, &[])?;

		let commit_object = repo.find_object(commit_id, Some(git2::ObjectType::Commit))?;
		repo.reset(&commit_object, ResetType::Hard, None)?;

		Ok(())
	}
}

pub struct GitHub;
impl GitHub {
	const GITHUB: &'static str = "github.com";
	pub async fn get_latest_releases(api_url: String) -> Result<Vec<Release>> {
		static APP_USER_AGENT: &str =
			concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
		let client = reqwest::ClientBuilder::new().user_agent(APP_USER_AGENT).build()?;
		let response = client.get(api_url).send().await?;
		Ok(response.json::<Vec<Release>>().await?)
	}

	pub async fn get_commit_sha_from_release(api_url: String) -> Result<String> {
		static APP_USER_AGENT: &str =
			concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

		let client = reqwest::ClientBuilder::new().user_agent(APP_USER_AGENT).build()?;

		let response = client.get(api_url).send().await?;
		let value = response.json::<serde_json::Value>().await?;
		let commit = value
			.get("object")
			.and_then(|v| v.get("sha"))
			.and_then(|v| v.as_str())
			.map(|v| v.to_owned())
			.ok_or(Error::Git("the github release tag sha was not found".to_string()))?;
		Ok(commit)
	}

	pub fn url_api_releases(repo: &Url) -> Result<String> {
		Ok(format!(
			"https://api.github.com/repos/{}/{}/releases",
			Self::org(repo)?,
			Self::name(repo)?
		))
	}
	pub fn url_api_tag_information(repo: &Url, tag_name: &str) -> Result<String> {
		Ok(format!(
			"https://api.github.com/repos/{}/{}/git/ref/tags/{}",
			Self::org(repo)?,
			Self::name(repo)?,
			tag_name
		))
	}

	fn org(repo: &Url) -> Result<&str> {
		let path_segments = repo
			.path_segments()
			.map(|c| c.collect::<Vec<_>>())
			.expect("repository must have path segments");
		Ok(path_segments.get(0).ok_or(Error::Git(
			"the organization (or user) is missing from the github url".to_string(),
		))?)
	}

	pub(crate) fn name(repo: &Url) -> Result<&str> {
		let path_segments = repo
			.path_segments()
			.map(|c| c.collect::<Vec<_>>())
			.expect("repository must have path segments");
		Ok(path_segments
			.get(1)
			.ok_or(Error::Git("the repository name is missing from the github url".to_string()))?)
	}

	pub(crate) fn release(repo: &Url, tag: &str, artifact: &str) -> String {
		format!("{}/releases/download/{tag}/{artifact}", repo.as_str())
	}
	pub(crate) fn convert_to_ssh_url(url: &Url) -> String {
		format!("git@{}:{}.git", url.host_str().unwrap_or(Self::GITHUB), &url.path()[1..])
	}
}

#[derive(serde::Deserialize)]
pub struct Release {
	pub tag_name: String,
	pub name: String,
	pub prerelease: bool,
	pub commit: Option<String>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use mockito::{Mock, Server};

	const BASE_PARACHAIN: &str = "https://github.com/r0gue-io/base-parachain";
	const POLKADOT_SDK: &str = "https://github.com/paritytech/polkadot-sdk";

	async fn releases_mock(mock_server: &mut Server, payload: String) -> Mock {
		mock_server
			.mock("GET", "/releases")
			.with_status(200)
			.with_header("content-type", "application/json")
			.with_body(payload)
			.create_async()
			.await
	}
	async fn tag_mock(mock_server: &mut Server, payload: String) -> Mock {
		mock_server
			.mock("GET", "/polkadot-v1.11.0")
			.with_status(200)
			.with_header("content-type", "application/json")
			.with_body(payload)
			.create_async()
			.await
	}

	#[tokio::test]
	async fn test_get_latest_releases() -> Result<(), Box<dyn std::error::Error>> {
		let mut mock_server = mockito::Server::new_async().await;

		let mut endpoint = mock_server.url();
		endpoint.push_str("/releases");
		let expected_payload = r#"[{
			"tag_name": "polkadot-v1.10.0",
			"name": "Polkadot v1.10.0",
			"prerelease": false
		  }]"#;
		let mock = releases_mock(&mut mock_server, expected_payload.to_string()).await;
		let latest_release = GitHub::get_latest_releases(endpoint).await?;
		assert_eq!(latest_release[0].name, "Polkadot v1.10.0");
		assert_eq!(latest_release[0].tag_name, "polkadot-v1.10.0");
		assert_eq!(latest_release[0].prerelease, false);
		mock.assert_async().await;
		Ok(())
	}

	#[tokio::test]
	async fn get_releases_with_commit_sha() -> Result<(), Box<dyn std::error::Error>> {
		let mut mock_server = mockito::Server::new_async().await;

		let mut endpoint = mock_server.url();
		endpoint.push_str("/polkadot-v1.11.0");
		let expected_payload = r#"{
			"ref": "refs/tags/polkadot-v1.11.0",
			"node_id": "REF_kwDOKDT1SrpyZWZzL3RhZ3MvcG9sa2Fkb3QtdjEuMTEuMA",
			"url": "https://api.github.com/repos/paritytech/polkadot-sdk/git/refs/tags/polkadot-v1.11.0",
			"object": {
				"sha": "0bb6249268c0b77d2834640b84cb52fdd3d7e860",
				"type": "commit",
				"url": "https://api.github.com/repos/paritytech/polkadot-sdk/git/commits/0bb6249268c0b77d2834640b84cb52fdd3d7e860"
			}
		  }"#;
		let mock = tag_mock(&mut mock_server, expected_payload.to_string()).await;
		let hash = GitHub::get_commit_sha_from_release(endpoint).await?;
		assert_eq!(hash, "0bb6249268c0b77d2834640b84cb52fdd3d7e860");
		mock.assert_async().await;
		Ok(())
	}

	#[test]
	fn test_get_releases_api_url() -> Result<(), Box<dyn std::error::Error>> {
		let url = Url::parse(POLKADOT_SDK)?;
		let api_url = GitHub::url_api_releases(&url)?;
		assert_eq!(api_url, "https://api.github.com/repos/paritytech/polkadot-sdk/releases");
		Ok(())
	}

	#[test]
	fn test_url_api_tag_information() -> Result<(), Box<dyn std::error::Error>> {
		let url = Url::parse(POLKADOT_SDK)?;
		let tag = "polkadot-v1.11.0";
		let api_url = GitHub::url_api_tag_information(&url, tag)?;
		assert_eq!(
			api_url,
			"https://api.github.com/repos/paritytech/polkadot-sdk/git/ref/tags/polkadot-v1.11.0"
		);
		Ok(())
	}

	#[test]
	fn test_parse_org() -> Result<(), Box<dyn std::error::Error>> {
		let url = Url::parse(BASE_PARACHAIN)?;
		let org = GitHub::org(&url)?;
		assert_eq!(org, "r0gue-io");
		Ok(())
	}

	#[test]
	fn test_parse_name() -> Result<(), Box<dyn std::error::Error>> {
		let url = Url::parse(BASE_PARACHAIN)?;
		let name = GitHub::name(&url)?;
		assert_eq!(name, "base-parachain");
		Ok(())
	}

	#[test]
	fn test_release_url() -> Result<(), Box<dyn std::error::Error>> {
		let repo = Url::parse(POLKADOT_SDK)?;
		let url = GitHub::release(&repo, &format!("polkadot-v1.9.0"), "polkadot");
		assert_eq!(url, format!("{}/releases/download/polkadot-v1.9.0/polkadot", POLKADOT_SDK));
		Ok(())
	}

	#[test]
	fn test_convert_to_ssh_url() {
		assert_eq!(
			GitHub::convert_to_ssh_url(&Url::parse(BASE_PARACHAIN).expect("valid repository url")),
			"git@github.com:r0gue-io/base-parachain.git"
		);
		assert_eq!(
			GitHub::convert_to_ssh_url(
				&Url::parse("https://github.com/paritytech/substrate-contracts-node")
					.expect("valid repository url")
			),
			"git@github.com:paritytech/substrate-contracts-node.git"
		);
		assert_eq!(
			GitHub::convert_to_ssh_url(
				&Url::parse("https://github.com/paritytech/frontier-parachain-template")
					.expect("valid repository url")
			),
			"git@github.com:paritytech/frontier-parachain-template.git"
		);
	}
}
