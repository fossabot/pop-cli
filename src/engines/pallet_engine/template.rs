use crate::{
	engines::generator::PalletItem,
	helpers::{resolve_pallet_path, sanitize},
};
use std::{fs, path::PathBuf};

// use super::{pallet_entry::AddPalletEntry, PalletEngine};

pub fn create_pallet_template(
	path: Option<String>,
	config: TemplatePalletConfig,
) -> anyhow::Result<()> {
	let target = resolve_pallet_path(path);
	// TODO : config.name might use `-` or use snake_case. We want to use pallet_template for the pallet dirs
	// and PalletTemplate for the runtime macro
	// TODO: this can be further polished (edge cases: no pallet prefix.)
	let pallet_name = config.name.clone();
	let pallet_path = target.join(pallet_name.clone());
	sanitize(&pallet_path)?;
	generate_pallet_structure(&target, &pallet_name)?;
	// todo let pallet_module_name = ... ;
	render_pallet(pallet_name, config, &pallet_path)?;
	Ok(())
}
#[derive(clap::Args, Clone)]
pub struct TemplatePalletConfig {
	#[arg(short, long, default_value_t = String::from("template"))]
	pub name: String,
	#[arg(short, long, default_value_t = String::from("author"))]
	pub authors: String,
	#[arg(short, long, default_value_t = String::from("description"))]
	pub description: String,
}
/// Generate a pallet folder and file structure
fn generate_pallet_structure(target: &PathBuf, pallet_name: &str) -> anyhow::Result<()> {
	use fs::{create_dir, File};
	let (pallet, src) = (target.join(pallet_name), target.join(pallet_name.to_string() + "/src"));
	create_dir(&pallet)?;
	create_dir(&src)?;
	File::create(format!("{}/Cargo.toml", pallet.display()))?;
	File::create(format!("{}/lib.rs", src.display()))?;
	File::create(format!("{}/benchmarking.rs", src.display()))?;
	File::create(format!("{}/tests.rs", src.display()))?;
	File::create(format!("{}/mock.rs", src.display()))?;
	Ok(())
}

fn render_pallet(
	pallet_name: String,
	config: TemplatePalletConfig,
	pallet_path: &PathBuf,
) -> anyhow::Result<()> {
	// let pallet_name = pallet_name.replace('-', "_");
	use crate::engines::generator::{
		PalletBenchmarking, PalletCargoToml, PalletLib, PalletMock, PalletTests,
	};
	// Todo `module` must be of the form Template if pallet_name : `pallet_template`
	let pallet: Vec<Box<dyn PalletItem>> = vec![
		Box::new(PalletCargoToml {
			name: pallet_name.clone(),
			authors: config.authors,
			description: config.description,
		}),
		Box::new(PalletLib {}),
		Box::new(PalletBenchmarking {}),
		Box::new(PalletMock { module: pallet_name.clone() }),
		Box::new(PalletTests { module: pallet_name }),
	];
	for item in pallet {
		item.execute(pallet_path)?;
	}
	Ok(())
}
