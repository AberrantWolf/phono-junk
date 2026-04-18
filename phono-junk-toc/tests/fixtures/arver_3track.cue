REM Synthetic CUE sheet matching the ARver 3-track test fixture:
REM   tracks=[75258, 54815, 205880], pregap=0, data=0
REM Expected absolute offsets: [150, 75408, 130223]; leadout 336103.
REM Source: https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py
FILE "arver_3track.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    INDEX 01 16:43:33
  TRACK 03 AUDIO
    INDEX 01 28:54:23
