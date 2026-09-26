Feature: The run's progress in the window
  A cut is minutes of ffmpeg with nothing to look at, so the window has to say where it has got
  to without inventing a number. What it may draw is decided by the engine's own readings, and the
  rules are here rather than in a component so they can be run.

  Scenario: the bar is the pass ffmpeg is running
    Given a pass of 100 seconds that is 25 seconds in
    When the bar is drawn
    Then the bar is 0.25 full

  Scenario: the bar cannot overfill when a pass runs past its own estimate
    Given a pass of 100 seconds that is 130 seconds in
    When the bar is drawn
    Then the bar is 1 full

  Scenario: a pass of unknown length does not invent a fraction
    Given a pass 25 seconds in whose length is not known
    When the bar is drawn
    Then there is no fraction to draw

  Scenario: a pass that has reported nothing does not invent a position
    Given a pass whose length is 100 seconds and which has reported nothing
    When the bar is drawn
    Then there is no fraction to draw

  Scenario: the estimate is what is left divided by the rate ffmpeg reported
    Given a pass of 100 seconds that is 25 seconds in at 5 times speed
    When the estimate is drawn
    Then 15 seconds are left

  Scenario: an estimate without a rate is not given
    Given a pass of 100 seconds that is 25 seconds in without a rate
    When the estimate is drawn
    Then there is no estimate to draw
