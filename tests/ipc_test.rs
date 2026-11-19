use ii::{
    err::Error,
    daemon::{
        Cmd, ERR_FLAG, Extmark, Header, MAX_PAYLOAD, RESP_FLAG, encode_dual_response, encode_error,
        encode_extmarks,
    },
};

#[test]
fn ipc_wire_codec_validation_and_dual_channel_framing() {
    let mut bad = Header::new(Cmd::Ping, 0);
    bad.magic = 0xDEAD_BEEF;
    assert_eq!(bad.validate(), Err(Error::BadMagic));

    let mut bad_ver = Header::new(Cmd::Ping, 0);
    bad_ver.version = 99;
    assert_eq!(bad_ver.validate(), Err(Error::BadVersion(99)));

    let mut huge = Header::new(Cmd::ComposeAnsi, MAX_PAYLOAD + 1);
    assert_eq!(
        huge.validate(),
        Err(Error::PayloadTooLarge(MAX_PAYLOAD + 1))
    );
    huge.payload_len = 16;
    huge.command = 0x00FF;
    assert_eq!(huge.validate(), Err(Error::UnknownCommand(0x00FF)));

    let mut out = Vec::new();
    encode_dual_response(Cmd::ComposeAnsi, b"APC_DATA", b"TEXT_DATA", &mut out);
    assert_eq!(out.len(), 32 + 4 + 8 + 9);
    let mut hdr_bytes = [0u8; 32];
    hdr_bytes.copy_from_slice(&out[..32]);
    let hdr = Header::from_le_bytes(&hdr_bytes);
    assert_eq!(hdr.command, (Cmd::ComposeAnsi as u16) | RESP_FLAG);
    assert_eq!(hdr.payload_len, 21);

    let mut err_buf = Vec::new();
    encode_error(Cmd::RenderFile, "failed", &mut err_buf);
    let mut eh_bytes = [0u8; 32];
    eh_bytes.copy_from_slice(&err_buf[..32]);
    let eh = Header::from_le_bytes(&eh_bytes);
    assert_eq!(eh.command, (Cmd::RenderFile as u16) | RESP_FLAG | ERR_FLAG);

    let ext = [Extmark {
        image_id: 42,
        start_row: 2,
        start_col: 0,
        end_row: 5,
        end_col: 10,
        cols: 10,
        rows: 3,
    }];
    let mut ext_out = Vec::new();
    encode_extmarks(&ext, &mut ext_out);
    assert_eq!(ext_out.len(), 16);
}
