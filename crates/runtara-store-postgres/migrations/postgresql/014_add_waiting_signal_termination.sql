-- Migration: Add 'waiting_signal' to termination_reason enum.
-- Stamped by the embedded runner when an invoke-shaped instance parks on an
-- on-signal wake (store-freeing WaitForSignal): it is the discriminator the
-- custom-signal waker requires before relaunching a suspended row — a
-- pause/breakpoint suspend carries no marker and must never be signal-woken
-- (its pause signal was already consumed, so a relaunch would replay PAST
-- the pause).
ALTER TYPE termination_reason ADD VALUE 'waiting_signal';
