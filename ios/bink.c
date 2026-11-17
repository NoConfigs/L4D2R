// Menu background on iOS: decodes the retail media/l4d2_background%02d.bik
// with a Bink-only FFmpeg (ios/build-ffmpeg.sh) into width x height RGBA,
// for src/menu_video.rs. Colour conversion matches the desktop ffmpeg pipe
// (-pix_fmt rgba: BT.601, limited range).
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/log.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct {
    AVFormatContext *format;
    AVCodecContext *decoder;
    AVPacket *packet;
    AVFrame *frame;
    int stream;
    int width;
    int height;
    int flushed;
} BinkVideo;

void l4d2r_bink_close(void *handle) {
    BinkVideo *video = handle;
    if (!video) return;
    av_frame_free(&video->frame);
    av_packet_free(&video->packet);
    avcodec_free_context(&video->decoder);
    avformat_close_input(&video->format);
    free(video);
}

void *l4d2r_bink_open(const char *path, int width, int height) {
    av_log_set_level(AV_LOG_ERROR);
    BinkVideo *video = calloc(1, sizeof *video);
    if (!video) return NULL;
    video->width = width;
    video->height = height;
    if (avformat_open_input(&video->format, path, NULL, NULL) < 0) goto fail;
    if (avformat_find_stream_info(video->format, NULL) < 0) goto fail;
    video->stream = av_find_best_stream(video->format, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (video->stream < 0) goto fail;
    AVCodecParameters *parameters = video->format->streams[video->stream]->codecpar;
    const AVCodec *codec = avcodec_find_decoder(parameters->codec_id);
    if (!codec) goto fail;
    video->decoder = avcodec_alloc_context3(codec);
    if (!video->decoder) goto fail;
    if (avcodec_parameters_to_context(video->decoder, parameters) < 0) goto fail;
    video->decoder->thread_count = 1;
    if (avcodec_open2(video->decoder, codec, NULL) < 0) goto fail;
    video->packet = av_packet_alloc();
    video->frame = av_frame_alloc();
    if (!video->packet || !video->frame) goto fail;
    return video;
fail:
    l4d2r_bink_close(video);
    return NULL;
}

static uint8_t clamp(int v) {
    return v < 0 ? 0 : v > 255 ? 255 : (uint8_t)v;
}

// YUV 4:2:0 (with or without alpha plane) to RGBA, nearest-neighbour scaled
// to the output size.
static void convert(const AVFrame *frame, uint8_t *rgba, int width, int height) {
    int fw = frame->width, fh = frame->height;
    for (int y = 0; y < height; y++) {
        int sy = (int)((int64_t)y * fh / height);
        const uint8_t *py = frame->data[0] + sy * frame->linesize[0];
        const uint8_t *pu = frame->data[1] + (sy >> 1) * frame->linesize[1];
        const uint8_t *pv = frame->data[2] + (sy >> 1) * frame->linesize[2];
        uint8_t *out = rgba + (size_t)y * width * 4;
        for (int x = 0; x < width; x++) {
            int sx = (int)((int64_t)x * fw / width);
            int c = 298 * (py[sx] - 16);
            int d = pu[sx >> 1] - 128;
            int e = pv[sx >> 1] - 128;
            out[0] = clamp((c + 409 * e + 128) >> 8);
            out[1] = clamp((c - 100 * d - 208 * e + 128) >> 8);
            out[2] = clamp((c + 516 * d + 128) >> 8);
            out[3] = 255;
            out += 4;
        }
    }
}

// Fills `rgba` with the next frame. Returns 0 at the end of the clip.
int l4d2r_bink_frame(void *handle, uint8_t *rgba) {
    BinkVideo *video = handle;
    if (!video) return 0;
    for (;;) {
        int got = avcodec_receive_frame(video->decoder, video->frame);
        if (got == 0) {
            int format = video->frame->format;
            if (format == AV_PIX_FMT_YUV420P || format == AV_PIX_FMT_YUVA420P) {
                convert(video->frame, rgba, video->width, video->height);
                av_frame_unref(video->frame);
                return 1;
            }
            av_frame_unref(video->frame);
            return 0;
        }
        if (got != AVERROR(EAGAIN)) return 0;
        if (video->flushed) return 0;
        int read = av_read_frame(video->format, video->packet);
        if (read < 0) {
            video->flushed = 1;
            avcodec_send_packet(video->decoder, NULL);
            continue;
        }
        if (video->packet->stream_index == video->stream) {
            avcodec_send_packet(video->decoder, video->packet);
        }
        av_packet_unref(video->packet);
    }
}
