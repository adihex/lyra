#ifndef LYRA_H
#define LYRA_H

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

/* ── playback engine (opaque handle) ── */
void  *lyra_engine_new(void);                     /* null on failure */
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
char  *lyra_engine_eq_response(const void *e);    /* JSON: {freqs, db} */
void   lyra_engine_free(void *e);

/* ── library DB (opaque handle) ── */
void  *lyra_lib_open(const char *path);           /* null on failure */
char  *lyra_lib_sync_dir(void *l, const char *dir); /* JSON SyncStats */
char  *lyra_lib_tracks(void *l);                  /* JSON array */
char  *lyra_lib_search(void *l, const char *q);   /* JSON array */
void   lyra_lib_free(void *l);

/* ── torrents (one global session) ── */
int   lyra_torrent_init(const char *download_dir);
int   lyra_torrent_add(const char *spec);          /* id >=0, <0 error; blocks on magnet metadata */
char *lyra_torrent_files(int id);                  /* JSON [{index,path,len}] */
char *lyra_torrent_stats(int id);                  /* JSON {progress_bytes,total_bytes,finished} */
char *lyra_torrent_probe(int id, int file_idx);    /* JSON {duration_secs,codec,sample_rate,channels} or NULL */
int   lyra_engine_play_torrent(void *e, int id, int file_idx);

#endif
