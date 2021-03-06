use crate::extractors::BragiQuery;
use crate::model::{Autocomplete, BragiError, FromWithLang};
use crate::routes::params;
use crate::{model, query, Context};
use actix_web::{Json, State};
use geojson::GeoJson;
use mimir::objects::Coord;
use std::time::Duration;

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
enum Type {
    #[serde(rename = "city")]
    City,
    #[serde(rename = "house")]
    House,
    #[serde(rename = "poi")]
    Poi,
    #[serde(rename = "public_transport:stop_area")]
    StopArea,
    #[serde(rename = "street")]
    Street,
}

impl Type {
    fn as_str(&self) -> &'static str {
        match self {
            &Type::City => "city",
            &Type::House => "house",
            &Type::Poi => "poi",
            &Type::StopArea => "public_transport:stop_area",
            &Type::Street => "street",
        }
    }
}

fn default_limit() -> u64 {
    10u64
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Params {
    q: String,
    #[serde(default)]
    pt_dataset: Vec<String>,
    #[serde(rename = "_all_data", default)]
    all_data: bool,
    //Note: for the moment we can't use an external struct and flatten it (https://github.com/nox/serde_urlencoded/issues/33)
    #[serde(default = "default_limit")]
    limit: u64,
    #[serde(default)]
    offset: u64,
    /// timeout in milliseconds
    timeout: Option<u64>,
    lat: Option<f64>,
    lon: Option<f64>,
    #[serde(default, rename = "type")]
    types: Vec<Type>,
    lang: Option<String>,
}

impl Params {
    fn types_as_str(&self) -> Vec<&str> {
        self.types.iter().map(Type::as_str).collect()
    }
    fn coord(&self) -> Result<Option<Coord>, BragiError> {
        match (self.lon, self.lat) {
            (Some(lon), Some(lat)) => Ok(Some(params::make_coord(lon, lat)?)),
            (None, None) => Ok(None),
            _ => Err(BragiError::InvalidParam(
                "you should provide a 'lon' AND a 'lat' parameter if you provide one of them",
            )),
        }
    }
    fn langs(&self) -> Vec<&str> {
        self.lang.iter().map(|l| l.as_str()).collect()
    }
    fn timeout(&self) -> Option<Duration> {
        self.timeout.map(Duration::from_millis)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JsonParams {
    shape: GeoJson,
}

impl JsonParams {
    fn get_es_shape(&self) -> Result<Vec<(f64, f64)>, model::BragiError> {
        match &self.shape {
            GeoJson::Feature(f) => {
                match &f.geometry {
                    Some(geom) => {
                        match &geom.value {
                            geojson::Value::Polygon(p) => {
                                match p.as_slice() {
                                    [p] => {
                                        Ok(p.iter()
                                            .filter_map(|c: &Vec<f64>| c.get(0..=1))
                                            .map(|c| (c[1], c[0])) // Note: the coord are inverted for ES
                                            .collect())
                                    }
                                    _ => Err(BragiError::InvalidShape(
                                        "only polygon without holes are supported",
                                    )), //only polygon without holes are supported by elasticsearch
                                }
                            }
                            _ => Err(BragiError::InvalidShape("only polygon are supported")),
                        }
                    }
                    None => Err(BragiError::InvalidShape("no geometry")),
                }
            }
            _ => Err(BragiError::InvalidShape("only 'feature' is supported")),
        }
    }
}

pub fn call_autocomplete(
    params: &Params,
    state: &Context,
    shape: Option<Vec<(f64, f64)>>,
) -> Result<Json<Autocomplete>, model::BragiError> {
    let langs = params.langs();
    let timeout = params::get_timeout(&params.timeout(), &state.max_es_timeout);
    let res = query::autocomplete(
        &params.q,
        &params
            .pt_dataset
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        params.all_data,
        params.offset,
        params.limit,
        params.coord()?,
        &state.es_cnx_string,
        shape,
        &params.types_as_str(),
        &langs,
        timeout,
    );
    res.map(|r| Autocomplete::from_with_lang(r, langs.into_iter().next()))
        .map(Json)
}

pub fn autocomplete(
    params: BragiQuery<Params>,
    state: State<Context>,
) -> Result<Json<Autocomplete>, model::BragiError> {
    call_autocomplete(&*params, &*state, None)
}

pub fn post_autocomplete(
    params: BragiQuery<Params>,
    state: State<Context>,
    json_params: Json<JsonParams>,
) -> Result<Json<Autocomplete>, model::BragiError> {
    call_autocomplete(&*params, &*state, Some(json_params.get_es_shape()?))
}
