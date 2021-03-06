// Copyright © 2016, Canal TP and/or its affiliates. All rights reserved.
//
// This file is part of Navitia,
//     the software to build cool stuff with public transport.
//
// Hope you'll enjoy and contribute to this project,
//     powered by Canal TP (www.canaltp.fr).
// Help us simplify mobility and open public transport:
//     a non ending quest to the responsive locomotion way of traveling!
//
// LICENCE: This program is free software; you can redistribute it
// and/or modify it under the terms of the GNU Affero General Public
// License as published by the Free Software Foundation, either
// version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful, but
// WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
// Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public
// License along with this program. If not, see
// <http://www.gnu.org/licenses/>.
//
// Stay tuned using
// twitter @navitia
// IRC #navitia on freenode
// https://groups.google.com/d/forum/navitia
// www.navitia.io

#[macro_use]
extern crate slog;
#[macro_use]
extern crate slog_scope;

use lazy_static::lazy_static;
use mimir::objects::Admin;
use mimir::rubber::{IndexSettings, Rubber};
use mimirsbrunn::addr_reader::import_addresses;
use mimirsbrunn::admin_geofinder::AdminGeoFinder;
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use structopt::StructOpt;

type AdminFromInsee = BTreeMap<String, Arc<Admin>>;

lazy_static! {
    static ref DEFAULT_NB_THREADS: String = num_cpus::get().to_string();
}

#[derive(Serialize, Deserialize)]
pub struct Bano {
    pub id: String,
    pub nb: String,
    pub street: String,
    pub zip: String,
    pub city: String,
    pub src: String,
    pub lat: f64,
    pub lon: f64,
}

impl Bano {
    pub fn insee(&self) -> &str {
        assert!(self.id.len() >= 5);
        self.id[..5].trim_left_matches('0')
    }
    pub fn fantoir(&self) -> &str {
        assert!(self.id.len() >= 10);
        &self.id[..10]
    }
    pub fn into_addr(
        self,
        admins_from_insee: &AdminFromInsee,
        admins_geofinder: &AdminGeoFinder,
    ) -> mimir::Addr {
        let street_label = format!("{} ({})", self.street, self.city);
        let addr_name = format!("{} {}", self.nb, self.street);
        let addr_label = format!("{} ({})", addr_name, self.city);
        let street_id = format!("street:{}", self.fantoir().to_string());
        let mut admins = admins_geofinder.get(&geo::Coordinate {
            x: self.lon,
            y: self.lat,
        });

        // If we have an admin corresponding to the INSEE, we know
        // that's the good one, thus we remove all the admins of its
        // level found by the geofinder, and add our admin.
        if let Some(admin) = admins_from_insee.get(self.insee()) {
            admins.retain(|a| a.level != admin.level);
            admins.push(admin.clone());
        }

        let weight = admins
            .iter()
            .find(|a| a.level == 8)
            .map_or(0., |a| a.weight);

        let street = mimir::Street {
            id: street_id,
            name: self.street,
            label: street_label.to_string(),
            administrative_regions: admins,
            weight: weight,
            zip_codes: vec![self.zip.clone()],
            coord: mimir::Coord::new(self.lon, self.lat),
            distance: None,
        };
        mimir::Addr {
            id: format!("addr:{};{}", self.lon, self.lat),
            name: addr_name,
            house_number: self.nb,
            street: street,
            label: addr_label,
            coord: mimir::Coord::new(self.lon, self.lat),
            weight: weight,
            zip_codes: vec![self.zip.clone()],
            distance: None,
        }
    }
}

fn index_bano<I>(
    cnx_string: &str,
    dataset: &str,
    files: I,
    nb_threads: usize,
    nb_shards: usize,
    nb_replicas: usize,
) -> Result<(), mimirsbrunn::Error>
where
    I: Iterator<Item = std::path::PathBuf>,
{
    let mut rubber = Rubber::new(cnx_string);
    rubber.initialize_templates()?;

    let admins = rubber
        .get_admins_from_dataset(dataset)
        .unwrap_or_else(|err| {
            info!(
                "Administratives regions not found in es db for dataset {}. (error: {})",
                dataset, err
            );
            vec![]
        });
    let admins_geofinder = admins.iter().cloned().collect();
    let admins_by_insee = admins
        .into_iter()
        .filter(|a| !a.insee.is_empty())
        .map(|mut a| {
            a.boundary = None; // to save some space we remove the admin boundary
            (a.insee.clone(), Arc::new(a))
        })
        .collect();

    let index_settings = IndexSettings {
        nb_shards: nb_shards,
        nb_replicas: nb_replicas,
    };

    import_addresses(
        &mut rubber,
        false,
        nb_threads,
        index_settings,
        dataset,
        files,
        move |b: Bano| b.into_addr(&admins_by_insee, &admins_geofinder),
    )
}

#[derive(StructOpt, Debug)]
struct Args {
    /// Bano files. Can be either a directory or a file.
    #[structopt(short = "i", long = "input", parse(from_os_str))]
    input: PathBuf,
    /// Elasticsearch parameters.
    #[structopt(
        short = "c",
        long = "connection-string",
        default_value = "http://localhost:9200/munin"
    )]
    connection_string: String,
    /// Name of the dataset.
    #[structopt(short = "d", long = "dataset", default_value = "fr")]
    dataset: String,
    /// Number of threads to use
    #[structopt(
        short = "t",
        long = "nb-threads",
        raw(default_value = "&DEFAULT_NB_THREADS")
    )]
    nb_threads: usize,
    /// Number of shards for the es index
    #[structopt(short = "s", long = "nb-shards", default_value = "5")]
    nb_shards: usize,
    /// Number of replicas for the es index
    #[structopt(short = "r", long = "nb-replicas", default_value = "1")]
    nb_replicas: usize,
}

fn run(args: Args) -> Result<(), mimirsbrunn::Error> {
    info!("importing bano into Mimir");
    if args.input.is_dir() {
        let paths: std::fs::ReadDir = fs::read_dir(&args.input)?;
        index_bano(
            &args.connection_string,
            &args.dataset,
            paths.map(|p| p.unwrap().path()),
            args.nb_threads,
            args.nb_shards,
            args.nb_replicas,
        )
    } else {
        index_bano(
            &args.connection_string,
            &args.dataset,
            std::iter::once(args.input),
            args.nb_threads,
            args.nb_shards,
            args.nb_replicas,
        )
    }
}
fn main() {
    mimirsbrunn::utils::launch_run(run);
}
