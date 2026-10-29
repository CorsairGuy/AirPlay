//! Общее для декодеров на libavcodec.

use ffmpeg_next as ff;

use crate::Error;

pub(crate) fn init() -> Result<(), Error> {
    ff::init().map_err(|e| Error::Init(format!("libavcodec: {e}")))?;
    // Ошибки пакетов libavcodec пишет в stderr сам; мы сообщаем их через Error.
    ff::util::log::set_level(ff::util::log::Level::Quiet);
    Ok(())
}

/// Кладёт `data` в `extradata` ещё не открытого контекста.
pub(crate) fn set_extradata(ctx: &mut ff::codec::Context, data: &[u8]) -> Result<(), Error> {
    // SAFETY: контекст наш и ещё не открыт; extradata выделяется
    // av_mallocz с запасом AV_INPUT_BUFFER_PADDING_SIZE, как требует
    // libavcodec, и освобождается им же вместе с контекстом (прежний
    // буфер, если был, освобождаем сами).
    unsafe {
        let raw = ctx.as_mut_ptr();
        let pad = ff::ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
        let buf = ff::ffi::av_mallocz(data.len() + pad) as *mut u8;
        if buf.is_null() {
            return Err(Error::Init("нет памяти".into()));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
        ff::ffi::av_free((*raw).extradata as *mut _);
        (*raw).extradata = buf;
        (*raw).extradata_size = data.len() as i32;
    }
    Ok(())
}
