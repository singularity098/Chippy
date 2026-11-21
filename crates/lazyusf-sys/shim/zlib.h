/* The one zlib function lazyusf2 uses (to notice self-modifying code), for
 * targets without zlib headers (Windows). Static, so it cannot clash with a
 * real zlib linked elsewhere. */
#ifndef CHIPPY_LAZYUSF_ZLIB_SHIM_H
#define CHIPPY_LAZYUSF_ZLIB_SHIM_H

#include <stddef.h>

static inline unsigned long adler32(unsigned long adler, const unsigned char *buf, size_t len)
{
    unsigned long a = adler & 0xffff, b = (adler >> 16) & 0xffff;
    while (len > 0) {
        /* 5552 bytes is the most that can be summed before b overflows 32 bits. */
        size_t n = len < 5552 ? len : 5552;
        len -= n;
        while (n--) {
            a += *buf++;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    return (b << 16) | a;
}

#endif
