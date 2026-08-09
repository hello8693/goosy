#pragma once

#include "goosy.h"
#include <cstdint>
#include <stdexcept>
#include <string>

namespace goosy {

struct RenderOptions {
    std::string song;
    std::string output;
    std::string lyrics;
    std::string background;
    std::string cover;
    std::string title;
    std::uint32_t width = 1920;
    std::uint32_t height = 1080;
    std::uint32_t fps = 30;
    std::string format = "auto";
    bool no_embedded_cover = false;
    bool no_audio = false;
};

inline std::string json_escape(const std::string &value) {
    std::string escaped;
    escaped.reserve(value.size() + 2);
    for (const char character : value) {
        switch (character) {
        case '\\': escaped += "\\\\"; break;
        case '"': escaped += "\\\""; break;
        case '\n': escaped += "\\n"; break;
        case '\r': escaped += "\\r"; break;
        case '\t': escaped += "\\t"; break;
        default: escaped += character; break;
        }
    }
    return escaped;
}

inline std::string render_request(const RenderOptions &options) {
    std::string request = "{\"song\":\"" + json_escape(options.song) +
        "\",\"output\":\"" + json_escape(options.output) + "\"";
    request += ",\"width\":" + std::to_string(options.width);
    request += ",\"height\":" + std::to_string(options.height);
    request += ",\"fps\":" + std::to_string(options.fps);
    request += ",\"format\":\"" + json_escape(options.format) + "\"";
    request += ",\"no_embedded_cover\":" + std::string(options.no_embedded_cover ? "true" : "false");
    request += ",\"no_audio\":" + std::string(options.no_audio ? "true" : "false");
    if (!options.lyrics.empty()) request += ",\"lyrics\":\"" + json_escape(options.lyrics) + "\"";
    if (!options.background.empty()) request += ",\"background\":\"" + json_escape(options.background) + "\"";
    if (!options.cover.empty()) request += ",\"cover\":\"" + json_escape(options.cover) + "\"";
    if (!options.title.empty()) request += ",\"title\":\"" + json_escape(options.title) + "\"";
    return request + "}";
}

inline void render(const RenderOptions &options) {
    const std::string request = render_request(options);
    if (const int code = goosy_render_json(request.c_str()); code != 0) {
        throw std::runtime_error(goosy_last_error() ? goosy_last_error() : "Goosy render failed");
    }
}

inline std::string parse_lyrics(const std::string &input, const std::string &format = "auto") {
    char *result = goosy_parse_lyrics_json(input.c_str(), format.c_str());
    if (!result) {
        throw std::runtime_error(goosy_last_error() ? goosy_last_error() : "Goosy parse failed");
    }
    std::string output(result);
    goosy_free_string(result);
    return output;
}

} // namespace goosy
