cmake_minimum_required(VERSION 3.13)
project(uxplay_raop_mirror_oracle C)

set(NO_X11_DEPS ON CACHE BOOL "Build the reference without X11" FORCE)
set(NO_MARCH_NATIVE ON CACHE BOOL "Use a portable reference oracle" FORCE)
add_subdirectory("${UXPLAY_REFERENCE}" uxplay-reference)

add_executable(raop_rtp_mirror_oracle raop_rtp_mirror_oracle.c)
target_include_directories(raop_rtp_mirror_oracle PRIVATE
    "${UXPLAY_REFERENCE}/lib"
    "${UXPLAY_REFERENCE}/lib/playfair"
    "${UXPLAY_REFERENCE}/lib/fairplay_v2"
    "${UXPLAY_REFERENCE}/lib/llhttp"
)
target_link_libraries(raop_rtp_mirror_oracle PRIVATE airplay)
