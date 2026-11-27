//! AirTunes DMAP track metadata parsing from `audio_set_metadata`.

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackMetadata {
    pub text: Vec<u8>,
    pub title: Option<Vec<u8>>,
    pub artist: Option<Vec<u8>>,
    pub album: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataError {
    ShortHeader,
    InvalidTag,
    InvalidRootTag,
    RootLengthMismatch,
    ChildLengthOverflow,
}

pub fn parse(data: &[u8]) -> Result<TrackMetadata, MetadataError> {
    let (root_tag, root_length, mut cursor) = read_header(data)?;
    if root_tag != *b"mlit" {
        return Err(MetadataError::InvalidRootTag);
    }
    if root_length != data.len() - cursor {
        return Err(MetadataError::RootLengthMismatch);
    }

    let mut result = TrackMetadata::default();
    while data.len() - cursor >= 8 {
        let (tag, length, value_start) = read_header(&data[cursor..])?;
        let value_end = value_start
            .checked_add(length)
            .filter(|end| *end <= data.len() - cursor)
            .ok_or(MetadataError::ChildLengthOverflow)?;
        let value = &data[cursor + value_start..cursor + value_end];
        let (label, track_field) = metadata_field(tag);
        if let Some(label) = label
            && !value.is_empty()
        {
            result.text.extend_from_slice(label.as_bytes());
            let visible = value.split(|byte| *byte == 0).next().unwrap_or_default();
            result.text.extend_from_slice(visible);
            result.text.push(b'\n');
            if let Some(field) = track_field {
                *field_mut(&mut result, field) = Some(visible.to_vec());
            }
        }
        cursor += value_end;
    }
    if cursor != data.len() {
        return Err(MetadataError::ShortHeader);
    }
    Ok(result)
}

fn read_header(data: &[u8]) -> Result<([u8; 4], usize, usize), MetadataError> {
    if data.len() < 8 {
        return Err(MetadataError::ShortHeader);
    }
    let mut tag = [0; 4];
    tag.copy_from_slice(&data[..4]);
    if !tag.iter().all(u8::is_ascii_alphabetic) {
        return Err(MetadataError::InvalidTag);
    }
    let length = u32::from_be_bytes(
        data[4..8]
            .try_into()
            .map_err(|_| MetadataError::ShortHeader)?,
    ) as usize;
    Ok((tag, length, 8))
}

fn metadata_field(tag: [u8; 4]) -> (Option<&'static str>, Option<TrackField>) {
    let (label, field) = match &tag {
        b"minm" => ("Title: ", Some(TrackField::Title)),
        b"asaa" => ("Album artist: ", None),
        b"asal" => ("Album: ", Some(TrackField::Album)),
        b"asar" => ("Artist: ", Some(TrackField::Artist)),
        b"ascm" => ("Comment: ", None),
        b"ascn" => ("Content description: ", None),
        b"ascp" => ("Composer: ", None),
        b"asct" => ("Category: ", None),
        b"assa" => ("Sort Artist: ", None),
        b"assc" => ("Sort Composer: ", None),
        b"assl" => ("Sort Album artist: ", None),
        b"assn" => ("Sort Name: ", None),
        b"asss" => ("Sort Series: ", None),
        b"assu" => ("Sort Album: ", None),
        b"asdt" => ("Description: ", None),
        b"asfm" => ("Format: ", None),
        b"asgn" => ("Genre: ", None),
        b"asky" => ("Keywords: ", None),
        b"aslc" => ("Long Content Description: ", None),
        _ => return (None, None),
    };
    (Some(label), field)
}

#[derive(Clone, Copy)]
enum TrackField {
    Title,
    Artist,
    Album,
}

fn field_mut(metadata: &mut TrackMetadata, field: TrackField) -> &mut Option<Vec<u8>> {
    match field {
        TrackField::Title => &mut metadata.title,
        TrackField::Artist => &mut metadata.artist,
        TrackField::Album => &mut metadata.album,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(tag: &[u8; 4], value: &[u8], output: &mut Vec<u8>) {
        output.extend_from_slice(tag);
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(value);
    }

    #[test]
    fn parses_source_supported_fields_and_track_overlay_values() {
        let mut body = Vec::new();
        item(b"minm", b"A song", &mut body);
        item(b"asar", b"An artist", &mut body);
        item(b"asal", b"An album", &mut body);
        item(b"asgn", b"Jazz", &mut body);
        let mut root = b"mlit".to_vec();
        root.extend_from_slice(&(body.len() as u32).to_be_bytes());
        root.extend_from_slice(&body);

        assert_eq!(
            parse(&root).unwrap(),
            TrackMetadata {
                text: b"Title: A song\nArtist: An artist\nAlbum: An album\nGenre: Jazz\n".to_vec(),
                title: Some(b"A song".to_vec()),
                artist: Some(b"An artist".to_vec()),
                album: Some(b"An album".to_vec()),
            }
        );
    }

    #[test]
    fn rejects_truncated_children_and_non_listingitem_root() {
        assert_eq!(parse(b"mlit\0\0\0\x01x"), Err(MetadataError::ShortHeader));
        assert_eq!(parse(b"nope\0\0\0\0"), Err(MetadataError::InvalidRootTag));
    }
}
