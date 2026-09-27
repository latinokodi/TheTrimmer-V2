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

  Scenario: captions move with the segment and are clamped at the marks
    Given a transcript with a cue across the in point and one across the out point
    When the segment is cut
    Then each cue is clamped to the mark it crossed
    And the file written is reported
