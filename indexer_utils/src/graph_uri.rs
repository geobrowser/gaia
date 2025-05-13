pub fn to_entity_id(uri: &str) -> Option<&str> {
    if let Some(end_index) = uri.find("?s=") {
        Some(&uri[..end_index])
    } else {
        Some(uri)
    }
}

pub fn to_space_id(uri: &str) -> Option<&str> {
    uri.split('?')
        .nth(1) // Get the query part
        .and_then(|query| {
            for param in query.split('&') {
                if let Some(value) = param.strip_prefix("s=") {
                    return Some(value);
                }
            }
            None
        })
}
