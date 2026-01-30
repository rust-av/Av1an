use crate::vs::plugins::Plugin;

pub struct JulekPlugin {}

impl Plugin for JulekPlugin {
    const PLUGIN_NAME: &'static str = "julek";
    const PLUGIN_ID: &'static str = "com.julek.plugin";
}
