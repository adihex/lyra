#ifndef LYRA_H
#define LYRA_H

#include <stdint.h>

/* lyra-ffi — C ABI for the Swift shell.
 * Strings returned by lyra_* are heap-allocated: free with lyra_string_free.
 */

const char *lyra_version(void);              /* static, do not free */
char *lyra_probe(const char *path);          /* JSON: format+stream+tags */
char *lyra_scan_dir(const char *path);       /* JSON array of LibraryTrack */
void  lyra_string_free(char *s);
/* ── remote: SPAKE2 pairing → pinned keys → Noise XX ── */
int   lyra_remote_init(void *e, const char *key_path);
int   lyra_remote_start(unsigned short port); /* 0 ok, 1 serve failed, 2 error */
char *lyra_remote_open_pairing(void);         /* JSON {code,fp} — free me */
int   lyra_remote_paired_count(void);
char *lyra_remote_devices(void);              /* JSON [{id,name}] — free me */
int   lyra_remote_revoke(const char *id);     /* 0 revoked, 1 unknown, 2 no remote */

/* ── playback engine (opaque handle) ── */
void  *lyra_engine_new(void);                     /* null on failure */
void  *lyra_engine_new_mode(int mode);            /* 0 compat, 1 HAL exclusive */
void  *lyra_engine_current(void);                 /* live handle — re-fetch after a mode switch */
int    lyra_engine_set_output_mode(int mode);     /* 0 ok, 1 unavailable — old engine kept */
int    lyra_engine_output_mode(void);             /* mode of the live engine */
int    lyra_engine_play_file(void *e, const char *path);
void   lyra_engine_pause(void *e);
void   lyra_engine_resume(void *e);
void   lyra_engine_stop(void *e);
void   lyra_engine_seek(void *e, double secs);
void   lyra_engine_set_volume(void *e, float v);
double lyra_engine_position(const void *e);
int    lyra_engine_is_playing(const void *e);
int    lyra_engine_can_resume(const void *e);   /* loaded & paused */
void   lyra_engine_set_band(void *e, int band, float freq, float q,
                            float gain_db, int peaking);
char  *lyra_engine_viz(const void *e);            /* JSON: bands/peak/clip */
unsigned long lyra_engine_viz_bands(const void *e, float *out, unsigned long n);

/* ── viz frame ABI (docs/VIZ-CONTRACT.md) ── */
typedef struct {
    float    bands[64];    /* normalized 0..1, log-spaced 20 Hz-20 kHz,
                              attack/decay ballistics applied           */
    float    wave_l[256];  /* strided-decimated recent PCM, newest last */
    float    wave_r[256];  /*   -1..1                                    */
    float    peak[2];      /* 0..1, PPM ballistics, L/R                  */
    float    rms[2];       /* 0..1, dBFS normalized (-60..0 -> 0..1)     */
    float    bass;         /* 0..1, mean energy of lowest ~4 bands       */
    float    beat;         /* 0..1, onset/transient pulse, ~150 ms decay */
    float    level;        /* 0..1, overall loudness (pulse modes)       */
    uint32_t clip;         /* sticky clip bitmask: bit0 L, bit1 R        */
    uint64_t seq;          /* monotonically increasing frame counter     */
} LyraVizFrame;            /* ~4.6 KB — one copy per UI frame            */

uint64_t lyra_engine_viz_frame(const void *e, LyraVizFrame *out);
/* returns seq; Swift skips redraw when seq is unchanged */
char  *lyra_engine_eq_response(const void *e);    /* JSON: {freqs, db} */
void   lyra_engine_free(void *e);

/* ── library DB (opaque handle) ── */
void  *lyra_lib_open(const char *path);           /* null on failure */
char  *lyra_lib_sync_dir(void *l, const char *dir); /* JSON SyncStats */
char  *lyra_lib_sync_files(void *l, const char *json); /* paths JSON -> SyncStats */
char  *lyra_lib_tracks(void *l);                  /* JSON array */
char  *lyra_lib_search(void *l, const char *q);   /* JSON array */
void   lyra_lib_free(void *l);

/* ── torrents (one global session) ── */
int   lyra_torrent_init(const char *download_dir);
int   lyra_torrent_add(const char *spec);          /* id >=0, <0 error; blocks on magnet metadata */
char *lyra_torrent_files(int id);                  /* JSON [{index,path,len}] */
char *lyra_torrent_stats(int id);                  /* JSON {progress_bytes,total_bytes,finished} */
int   lyra_torrent_remove(int id, int delete_files); /* 0 ok; delete_files!=0 also wipes downloaded data */
char *lyra_torrent_list(void);                     /* JSON [{id,name}] — session is the source of truth */
char *lyra_torrent_orphans(void);                  /* JSON [{name,bytes}] — unowned download_dir entries */
char *lyra_torrent_purge_orphans(void);            /* JSON {removed,bytes} */
char *lyra_torrent_probe(int id, int file_idx);    /* JSON {duration_secs,codec,sample_rate,channels} or NULL */
int   lyra_engine_play_torrent(void *e, int id, int file_idx);

/* ── torrent search (lyra-search): legal indexes, lossless-first ── */
void  *lyra_search_new(const char *data_dir);     /* provider caches under data_dir; null on failure */
void   lyra_search_free(void *s);
char  *lyra_search(void *s, const char *query_json);          /* SearchResponse JSON — free me; blocks, call off-main */
char  *lyra_search_resolve(void *s, const char *result_json); /* {result,files,addable:{kind:magnet|torrent_url|torrent_b64,…}} or {"error":…} */

#endif
