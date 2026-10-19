Feature: Where the sound stops
  A cut inside a word clips a consonant the picture cannot show, and the pause either side of the
  words is the only place worth landing on. Finding the pauses costs one audio decode, not one of
  the picture.

  Scenario: the pauses are read from the master's own silence
    Given silencedetect reporting a gap from 1.234s to 5.678s
    When the gaps are read
    Then there is one gap from 1.234s to 5.678s

  Scenario: a mark moves into the nearest pause
    Given a master at 25 fps and a pause from 4.0s to 6.0s
    When frame 99 is moved into the nearest pause
    Then the mark lands at frame 125

  Scenario: a mark with no pause in reach stays where it is
    Given a master at 25 fps and a pause from 40.0s to 60.0s
    When frame 99 is moved into the nearest pause
    Then the mark stays at frame 99

  Scenario: a source with no sound has no pauses
    Given silencedetect reporting nothing
    When the gaps are read
    Then there are no gaps
