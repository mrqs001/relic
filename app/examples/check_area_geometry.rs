//! Read-only publisher geometry check; no workspace, configuration or network.
fn main() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: check_area_geometry GEOJSON")?;
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024 {
        return Err("Geometry input exceeds 32 MiB".into());
    }
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    for f in raw["features"].as_array().ok_or("Missing features")? {
        match relic_app::area_geometry::AreaGeometry::parse_marine(
            &f["geometry"],
            relic_app::area_geometry::MAX_VERTICES,
            &std::sync::atomic::AtomicBool::new(false),
        ) {
            Ok(g) => println!(
                "{}",
                serde_json::json!({"id":f["id"],"valid":true,"vertices":g.vertices,"bounds":g.bounds})
            ),
            Err(e) => println!(
                "{}",
                serde_json::json!({"id":f["id"],"valid":false,"reason":e.message})
            ),
        }
    }
    Ok(())
}
