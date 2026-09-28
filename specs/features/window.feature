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

  Scenario: the log shows the newest line first
    Given a run that has said "source", "head", "body"
    When the log is drawn
    Then the first line is "body"
    And the last line is "source"

  Scenario: the plan line is a forecast, because nothing has been cut yet
    Given a range that will re-encode 37 frames and copy 835
    When the plan line is drawn
    Then it says "head patch — will re-encode 37, copy 835"
    And it does not claim anything has already happened

  Scenario: a range that can be copied whole says so as a forecast too
    Given a range of 900 frames that will be copied untouched
    When the plan line is drawn
    Then it says "lossless copy — will copy all 900 frames untouched"
    And it does not claim anything has already happened

  Scenario: a range with no keyframe says the whole of it is coming
    Given a range of 900 frames that will be re-encoded whole
    When the plan line is drawn
    Then it says "full re-encode — no keyframe in this range, so all 900 will be re-encoded"
    And it does not claim anything has already happened

  Scenario: lines that are a record say they are a record
    Given a run that has ended holding 12 line(s)
    When the progress panel is drawn
    Then its note reads "from the last run"

  Scenario: lines being written now are not labelled as a record
    Given a run that is still going holding 12 line(s)
    When the progress panel is drawn
    Then it has no note

  Scenario: an empty panel has nothing to label
    Given a run that has ended holding 0 line(s)
    When the progress panel is drawn
    Then it has no note
