Feature: Verification
  A cut nobody measured is a cut nobody can vouch for. What matters as much as the measurement is
  that "we looked and it was right" and "we did not look" never look alike.

  Scenario Outline: a verdict never means two things at once
    Given a check that <state>
    When the window is told about it
    Then it reads "<verdict>"

    Examples:
      | state            | verdict     |
      | passed           | passed      |
      | failed           | failed      |
      | was not run      | not checked |

  Scenario: alignment samples avoid the head and the overshoot
    Given a head patch of 100 frames in a segment of 3000 frames
    When I choose the frames to sample
    Then every sample is after the re-encoded head
    And every sample sits inside the range that was asked for

  Scenario: a short segment still gets a sample
    Given a head patch of 10 frames in a segment of 30 frames
    When I choose the frames to sample
    Then at least one sample is chosen

  Scenario: disagreeing samples are not corrected on
    Given samples that report -1, 0 and 2 frames
    When I ask what offset they agree on
    Then there is no agreed offset

  Scenario: agreeing samples are corrected on
    Given samples that all report -2 frames
    When I ask what offset they agree on
    Then the agreed offset is -2

  Scenario: samples that could not be measured are not corrected on
    Given samples that could not be measured
    When I ask what offset they agree on
    Then there is no agreed offset

  Scenario: captions move with the segment and are clamped at the marks
    Given a transcript with a cue across the in point and one across the out point
    When the segment is cut
    Then each cue is clamped to the mark it crossed
    And the file written is reported
