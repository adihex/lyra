#ifndef LYRA_H
#define LYRA_H

/* lyra-ffi — C ABI for the Swift shell.
 * Strings returned by lyra_* are heap-allocated: free with lyra_string_free.
 */

const char *lyra_version(void);              /* static, do not free */
char *lyra_probe(const char *path);          /* JSON: format+stream+tags */
void  lyra_string_free(char *s);
int   lyra_remote_start(unsigned short port); /* 0 ok, 1 port taken, 2 error */

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
char  *lyra_engine_viz(const void *e);            /* JSON: bands/peak/clip */
void   lyra_engine_free(void *e);

#endif
