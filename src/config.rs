use std::fs;
use std::path::PathBuf;
use std::process;

pub fn config_dir() -> PathBuf {
    let dir = home_dir().join(".config").join("cl");
    fs::create_dir_all(&dir).ok();
    dir
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .expect("HOME not set")
}

pub fn save_token(key: &str) {
    let path = config_dir().join("linear_token");
    fs::write(&path, key.trim()).unwrap_or_else(|e| {
        eprintln!("Error writing token to {}: {e}", path.display());
        process::exit(1);
    });
    println!("Token saved to {}", path.display());
}

pub fn load_token() -> String {
    if let Ok(token) = fs::read_to_string(config_dir().join("linear_token")) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return token;
        }
    }
    if let Ok(token) = std::env::var("LINEAR_API_KEY") {
        return token;
    }
    eprintln!("Error: No Linear API key found. Run `cl login <key>` or set LINEAR_API_KEY.");
    process::exit(1);
}
