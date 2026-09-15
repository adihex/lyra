#ifndef LYRA_H
#define LYRA_H

/* lyra-ffi — C ABI for the Swift shell.
 * Strings returned by lyra_* are heap-allocated: free with lyra_string_free.
 */

const char *lyra_version(void);              /* static, do not free */
char *lyra_probe(const char *path);          /* JSON: format+stream+tags */
void  lyra_string_free(char *s);
int   lyra_remote_start(unsigned short port); /* 0 ok, 1 port taken, 2 error */

#endif
