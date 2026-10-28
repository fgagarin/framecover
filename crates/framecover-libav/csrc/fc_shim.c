/*
 * Flat C interface over libav* for framecover-libav (see src/lib.rs).
 *
 * Two parts:
 *  - fc_tool_run: runs the ffmpeg / ffprobe front ends linked from
 *    libfftools.a (`ffmpeg_main` / `ffprobe_main`) inside this process,
 *    resetting their global state between runs;
 *  - fc_decoder_*: decodes the first non-attached-picture video stream to
 *    RGB24 the way `ffmpeg -noautorotate [-ss S] -i IN -map 0:V:0 [-vf F]
 *    -pix_fmt rgb24` does, frame by frame.
 */

#define _POSIX_C_SOURCE 200809L /* sigaction under -std=c11 */

#include <errno.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavfilter/avfilter.h>
#include <libavfilter/buffersink.h>
#include <libavfilter/buffersrc.h>
#include <libavformat/avformat.h>
#include <libavutil/avstring.h>
#include <libavutil/opt.h>
#include <libavutil/pixdesc.h>

/* ------------------------------------------------------------------------ */
/* Logging                                                                   */
/* ------------------------------------------------------------------------ */

typedef void (*fc_log_fn)(int level, const char *line);

static fc_log_fn log_sink;

static void log_callback(void *avcl, int level, const char *fmt, va_list vl)
{
    static _Thread_local int print_prefix = 1;
    char line[2048];
    fc_log_fn sink = log_sink;

    if (level > av_log_get_level() || !sink)
        return;
    av_log_format_line2(avcl, level, fmt, vl, line, sizeof(line), &print_prefix);
    sink(level, line);
}

/* Route every av_log line to `sink` (called once, before anything else). */
void fc_init(fc_log_fn sink, int level)
{
    log_sink = sink;
    av_log_set_level(level);
    av_log_set_callback(log_callback);
}

/* ------------------------------------------------------------------------ */
/* In-process ffmpeg / ffprobe                                               */
/* ------------------------------------------------------------------------ */

int ffmpeg_main(int argc, char **argv);
int ffprobe_main(int argc, char **argv);

/*
 * build-ffmpeg.sh moves each front end's writable data into its own
 * sections (fc_<tool>_data / fc_<tool>_bss), so the linker defines
 * __start_ / __stop_ symbols around them. Their pristine contents are
 * restored before every run: the tools keep option values and array
 * counters in globals and assume a fresh process.
 */
#define SECTION_BOUNDS(tool)                                                  \
    extern char __start_fc_##tool##_data[] __attribute__((weak));             \
    extern char __stop_fc_##tool##_data[] __attribute__((weak));              \
    extern char __start_fc_##tool##_bss[] __attribute__((weak));              \
    extern char __stop_fc_##tool##_bss[] __attribute__((weak));
SECTION_BOUNDS(ffmpeg)
SECTION_BOUNDS(ffprobe)

typedef struct ToolState {
    int (*main)(int, char **);
    char *data, *data_end, *bss, *bss_end;
    void *snapshot;
} ToolState;

static ToolState tools[2] = {
    { ffmpeg_main, __start_fc_ffmpeg_data, __stop_fc_ffmpeg_data,
      __start_fc_ffmpeg_bss, __stop_fc_ffmpeg_bss, NULL },
    { ffprobe_main, __start_fc_ffprobe_data, __stop_fc_ffprobe_data,
      __start_fc_ffprobe_bss, __stop_fc_ffprobe_bss, NULL },
};

static const int saved_signals[] = { SIGINT, SIGTERM, SIGQUIT, SIGPIPE,
#ifdef SIGXCPU
                                     SIGXCPU,
#endif
};
#define NB_SAVED_SIGNALS (sizeof(saved_signals) / sizeof(saved_signals[0]))

/*
 * Run tool 0 (ffmpeg) or 1 (ffprobe) with `argv` (argv[0] included) and
 * return its exit code, or -1 if its state cannot be reset. Not reentrant:
 * the caller serialises all runs.
 */
int fc_tool_run(int tool, int argc, char **argv)
{
    ToolState *t;
    struct sigaction old[NB_SAVED_SIGNALS];
    int level, ret;

    if (tool < 0 || tool > 1)
        return -1;
    t = &tools[tool];
    if (!t->data || !t->bss)
        return -1;
    if (!t->snapshot) {
        t->snapshot = malloc(t->data_end - t->data + 1);
        if (!t->snapshot)
            return -1;
        memcpy(t->snapshot, t->data, t->data_end - t->data);
    } else {
        memcpy(t->data, t->snapshot, t->data_end - t->data);
    }
    memset(t->bss, 0, t->bss_end - t->bss);

    for (size_t i = 0; i < NB_SAVED_SIGNALS; i++)
        sigaction(saved_signals[i], NULL, &old[i]);
    level = av_log_get_level();

    ret = t->main(argc, argv);

    av_log_set_level(level);
    av_log_set_flags(0);
    av_log_set_callback(log_callback);
    for (size_t i = 0; i < NB_SAVED_SIGNALS; i++)
        sigaction(saved_signals[i], &old[i], NULL);
    return ret;
}

/* ------------------------------------------------------------------------ */
/* Codec lookups                                                             */
/* ------------------------------------------------------------------------ */

/*
 * Iterate over the encoders (encoders != 0) or decoders of this build, as
 * `ffmpeg -encoders` / `-decoders` list them. Start with *opaque = NULL;
 * returns 0 once there are no more. `type` is 'V', 'A', 'S' or 'D'.
 */
int fc_codec_next(void **opaque, int encoders, const char **name, const char **long_name,
                  char *type)
{
    const AVCodec *codec;
    while ((codec = av_codec_iterate(opaque))) {
        if (encoders ? !av_codec_is_encoder(codec) : !av_codec_is_decoder(codec))
            continue;
        *name = codec->name;
        *long_name = codec->long_name ? codec->long_name : "";
        switch (codec->type) {
        case AVMEDIA_TYPE_VIDEO: *type = 'V'; break;
        case AVMEDIA_TYPE_AUDIO: *type = 'A'; break;
        case AVMEDIA_TYPE_SUBTITLE: *type = 'S'; break;
        default: *type = 'D'; break;
        }
        return 1;
    }
    return 0;
}

/* ------------------------------------------------------------------------ */
/* Frame decoder                                                             */
/* ------------------------------------------------------------------------ */

typedef struct FcDecoder {
    AVFormatContext *fmt;
    AVCodecContext *dec;
    AVPacket *pkt;
    AVFrame *frame;
    AVFrame *out;
    int stream;
    AVRational time_base;
    char *filter;

    AVFilterGraph *graph;
    AVFilterContext *src, *sink;
    int graph_w, graph_h, graph_format;
    enum AVColorSpace graph_space;
    enum AVColorRange graph_range;

    int64_t threshold;
    int input_done, decoder_done, graph_done;

    uint8_t *rgb;
    size_t rgb_size;
} FcDecoder;

typedef struct FcFrame {
    int64_t pts;
    int width, height;
    /* Tightly packed RGB24, valid until the next fc_decoder_next call. */
    const uint8_t *data;
} FcFrame;

static void set_error(char *err, size_t len, const char *what, int code)
{
    char buf[AV_ERROR_MAX_STRING_SIZE] = { 0 };
    if (!err || !len)
        return;
    if (code < 0) {
        av_strerror(code, buf, sizeof(buf));
        snprintf(err, len, "%s: %s", what, buf);
    } else {
        snprintf(err, len, "%s", what);
    }
}

void fc_decoder_close(FcDecoder *d)
{
    if (!d)
        return;
    avfilter_graph_free(&d->graph);
    av_frame_free(&d->frame);
    av_frame_free(&d->out);
    av_packet_free(&d->pkt);
    avcodec_free_context(&d->dec);
    avformat_close_input(&d->fmt);
    av_free(d->filter);
    av_free(d->rgb);
    av_free(d);
}

/*
 * Open `path`; `filter` (may be NULL) is the -vf chain placed before the
 * conversion to RGB24. Returns NULL with a message in `err` on failure.
 */
FcDecoder *fc_decoder_open(const char *path, const char *filter, char *err, size_t err_len)
{
    FcDecoder *d = av_mallocz(sizeof(*d));
    const AVCodec *codec;
    AVStream *st;
    AVDictionary *opts = NULL;
    int ret;

    if (!d) {
        set_error(err, err_len, "out of memory", 0);
        return NULL;
    }
    d->stream = -1;
    d->threshold = INT64_MIN;
    if (filter && *filter && !(d->filter = av_strdup(filter)))
        goto nomem;

    if ((ret = avformat_open_input(&d->fmt, path, NULL, NULL)) < 0) {
        set_error(err, err_len, "could not open input", ret);
        goto fail;
    }
    if ((ret = avformat_find_stream_info(d->fmt, NULL)) < 0) {
        set_error(err, err_len, "could not read stream info", ret);
        goto fail;
    }
    /* `0:V:0`: the first video stream that is not a cover picture. */
    for (unsigned i = 0; i < d->fmt->nb_streams; i++) {
        AVStream *s = d->fmt->streams[i];
        if (s->codecpar->codec_type == AVMEDIA_TYPE_VIDEO &&
            !(s->disposition & (AV_DISPOSITION_ATTACHED_PIC | AV_DISPOSITION_TIMED_THUMBNAILS))) {
            d->stream = i;
            break;
        }
    }
    if (d->stream < 0) {
        set_error(err, err_len, "no video stream", 0);
        goto fail;
    }
    for (unsigned i = 0; i < d->fmt->nb_streams; i++)
        if ((int)i != d->stream)
            d->fmt->streams[i]->discard = AVDISCARD_ALL;

    st = d->fmt->streams[d->stream];
    d->time_base = st->time_base;
    codec = avcodec_find_decoder(st->codecpar->codec_id);
    if (!codec) {
        set_error(err, err_len, "no decoder for the video stream", 0);
        goto fail;
    }
    if (!(d->dec = avcodec_alloc_context3(codec)))
        goto nomem;
    if ((ret = avcodec_parameters_to_context(d->dec, st->codecpar)) < 0) {
        set_error(err, err_len, "could not configure the decoder", ret);
        goto fail;
    }
    d->dec->pkt_timebase = st->time_base;
    av_dict_set(&opts, "threads", "auto", 0);
    ret = avcodec_open2(d->dec, codec, &opts);
    av_dict_free(&opts);
    if (ret < 0) {
        set_error(err, err_len, "could not open the decoder", ret);
        goto fail;
    }
    if (!(d->pkt = av_packet_alloc()) || !(d->frame = av_frame_alloc()) ||
        !(d->out = av_frame_alloc()))
        goto nomem;
    return d;

nomem:
    set_error(err, err_len, "out of memory", 0);
fail:
    fc_decoder_close(d);
    return NULL;
}

void fc_decoder_time_base(const FcDecoder *d, int *num, int *den)
{
    *num = d->time_base.num;
    *den = d->time_base.den;
}

/* Container start time in microseconds (0 when unknown, as ffmpeg treats it). */
int64_t fc_decoder_start_us(const FcDecoder *d)
{
    return d->fmt->start_time == AV_NOPTS_VALUE ? 0 : d->fmt->start_time;
}

/*
 * Seek like an input `-ss seek_us`: to the keyframe before seek_us plus the
 * container start time. Frames with timestamps below `threshold` (stream
 * ticks; INT64_MIN keeps all) are dropped, like ffmpeg's accurate-seek trim.
 * Call once, before the first fc_decoder_next.
 */
int fc_decoder_seek(FcDecoder *d, int64_t seek_us, int64_t threshold)
{
    d->threshold = threshold;
    if (seek_us <= 0)
        return 0;
    int64_t ts = seek_us;
    if (d->fmt->start_time != AV_NOPTS_VALUE)
        ts += d->fmt->start_time;
    /* ffmpeg ignores seek failures with a warning and decodes from the start. */
    if (avformat_seek_file(d->fmt, -1, INT64_MIN, ts, ts, 0) < 0)
        av_log(NULL, AV_LOG_WARNING, "could not seek to position %.3f\n", ts / 1e6);
    return 0;
}

static int configure_graph(FcDecoder *d, const AVFrame *f)
{
    AVFilterInOut *in = NULL, *out = NULL;
    AVFilterContext *format = NULL;
    AVBufferSrcParameters *par;
    char args[512];
    int ret;

    avfilter_graph_free(&d->graph);
    if (!(d->graph = avfilter_graph_alloc()))
        return AVERROR(ENOMEM);

    snprintf(args, sizeof(args),
             "video_size=%dx%d:pix_fmt=%d:time_base=%d/%d:pixel_aspect=%d/%d:colorspace=%d:range=%d",
             f->width, f->height, f->format, d->time_base.num, d->time_base.den,
             f->sample_aspect_ratio.num, FFMAX(f->sample_aspect_ratio.den, 1),
             f->colorspace, f->color_range);
    if ((ret = avfilter_graph_create_filter(&d->src, avfilter_get_by_name("buffer"), "in",
                                            args, NULL, d->graph)) < 0)
        return ret;
    if (f->hw_frames_ctx) {
        if (!(par = av_buffersrc_parameters_alloc()))
            return AVERROR(ENOMEM);
        par->hw_frames_ctx = f->hw_frames_ctx;
        ret = av_buffersrc_parameters_set(d->src, par);
        av_free(par);
        if (ret < 0)
            return ret;
    }
    /* The CLI's output side: `format=pix_fmts=rgb24` before the sink. */
    if ((ret = avfilter_graph_create_filter(&format, avfilter_get_by_name("format"), "format",
                                            "pix_fmts=rgb24", NULL, d->graph)) < 0)
        return ret;
    if ((ret = avfilter_graph_create_filter(&d->sink, avfilter_get_by_name("buffersink"), "out",
                                            NULL, NULL, d->graph)) < 0)
        return ret;
    if ((ret = avfilter_link(format, 0, d->sink, 0)) < 0)
        return ret;

    if (d->filter) {
        if (!(in = avfilter_inout_alloc()) || !(out = avfilter_inout_alloc())) {
            ret = AVERROR(ENOMEM);
            goto end;
        }
        out->name = av_strdup("in");
        out->filter_ctx = d->src;
        in->name = av_strdup("out");
        in->filter_ctx = format;
        if ((ret = avfilter_graph_parse_ptr(d->graph, d->filter, &in, &out, NULL)) < 0)
            goto end;
    } else if ((ret = avfilter_link(d->src, 0, format, 0)) < 0) {
        goto end;
    }
    ret = avfilter_graph_config(d->graph, NULL);
    d->graph_w = f->width;
    d->graph_h = f->height;
    d->graph_format = f->format;
    d->graph_space = f->colorspace;
    d->graph_range = f->color_range;
end:
    avfilter_inout_free(&in);
    avfilter_inout_free(&out);
    return ret;
}

/* Feed the next decoded frame at or after the threshold into the graph. */
static int feed(FcDecoder *d)
{
    for (;;) {
        int ret = avcodec_receive_frame(d->dec, d->frame);
        if (ret == 0) {
            int64_t pts = d->frame->best_effort_timestamp;
            d->frame->pts = pts;
            if (d->threshold != INT64_MIN && (pts == AV_NOPTS_VALUE || pts < d->threshold)) {
                av_frame_unref(d->frame);
                continue;
            }
            if (!d->graph || d->frame->width != d->graph_w || d->frame->height != d->graph_h ||
                d->frame->format != d->graph_format || d->frame->colorspace != d->graph_space ||
                d->frame->color_range != d->graph_range) {
                if ((ret = configure_graph(d, d->frame)) < 0) {
                    av_frame_unref(d->frame);
                    return ret;
                }
            }
            ret = av_buffersrc_add_frame_flags(d->src, d->frame, 0);
            av_frame_unref(d->frame);
            return ret;
        }
        if (ret == AVERROR_EOF) {
            d->decoder_done = 1;
            if (d->graph)
                return av_buffersrc_add_frame_flags(d->src, NULL, 0);
            d->graph_done = 1;
            return 0;
        }
        if (ret != AVERROR(EAGAIN))
            return ret;

        if (d->input_done)
            return AVERROR_BUG;
        ret = av_read_frame(d->fmt, d->pkt);
        if (ret == AVERROR_EOF) {
            d->input_done = 1;
            ret = avcodec_send_packet(d->dec, NULL);
        } else if (ret < 0) {
            return ret;
        } else {
            if (d->pkt->stream_index == d->stream)
                ret = avcodec_send_packet(d->dec, d->pkt);
            av_packet_unref(d->pkt);
            /* Like ffmpeg, keep decoding past corrupt packets. */
            if (ret == AVERROR_INVALIDDATA)
                ret = 0;
        }
        if (ret < 0 && ret != AVERROR(EAGAIN))
            return ret;
    }
}

/*
 * Next RGB24 frame: 0 with `out` filled, 1 at the end of the stream, or a
 * negative AVERROR with a message in `err`.
 */
int fc_decoder_next(FcDecoder *d, FcFrame *out, char *err, size_t err_len)
{
    for (;;) {
        if (d->graph_done)
            return 1;
        if (d->graph) {
            av_frame_unref(d->out);
            int ret = av_buffersink_get_frame(d->sink, d->out);
            if (ret == 0) {
                if (d->out->format != AV_PIX_FMT_RGB24) {
                    set_error(err, err_len, "filter graph did not produce rgb24", 0);
                    return AVERROR_BUG;
                }
                /* Pack the rows into our own buffer: the frame's may be shared. */
                size_t row = (size_t)d->out->width * 3;
                size_t size = row * d->out->height;
                if (size > d->rgb_size) {
                    av_freep(&d->rgb);
                    d->rgb_size = 0;
                    if (!(d->rgb = av_malloc(size))) {
                        set_error(err, err_len, "out of memory", 0);
                        return AVERROR(ENOMEM);
                    }
                    d->rgb_size = size;
                }
                for (int y = 0; y < d->out->height; y++)
                    memcpy(d->rgb + y * row, d->out->data[0] + (size_t)y * d->out->linesize[0], row);
                out->pts = d->out->pts;
                out->width = d->out->width;
                out->height = d->out->height;
                out->data = d->rgb;
                return 0;
            }
            if (ret == AVERROR_EOF) {
                d->graph_done = 1;
                return 1;
            }
            if (ret != AVERROR(EAGAIN)) {
                set_error(err, err_len, "filtering failed", ret);
                return ret;
            }
            if (d->decoder_done) {
                d->graph_done = 1;
                return 1;
            }
        }
        int ret = feed(d);
        if (ret < 0) {
            set_error(err, err_len, "decoding failed", ret);
            return ret;
        }
    }
}
