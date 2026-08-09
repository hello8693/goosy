#ifndef GOOSY_H
#define GOOSY_H

#ifdef __cplusplus
extern "C" {
#endif

const char *goosy_version(void);
const char *goosy_last_error(void);
int goosy_render_json(const char *request_json);
char *goosy_parse_lyrics_json(const char *input, const char *format);
void goosy_free_string(char *value);

#ifdef __cplusplus
}
#endif

#endif
