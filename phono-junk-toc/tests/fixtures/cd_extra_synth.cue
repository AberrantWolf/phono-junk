REM Synthetic CD-Extra CUE: 3 audio tracks matching ARver 3-track, plus a
REM trailing MODE2 data track at (audio_leadout + 11400) absolute sectors.
REM Expected phono-junk Toc after CD-Extra correction is identical to
REM arver_3track.cue — that is the win condition of the -11,400 correction.
REM Source: https://musicbrainz.org/doc/Disc_ID_Calculation (multi-session)
REM         + https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py
FILE "cd_extra_synth.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    INDEX 01 16:43:33
  TRACK 03 AUDIO
    INDEX 01 28:54:23
  TRACK 04 MODE2/2352
    INDEX 01 77:11:28
