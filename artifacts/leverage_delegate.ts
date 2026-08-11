/**
 * Program IDL in camelCase format in order to be used in JS/TS.
 *
 * Note that this is only a type helper and is not the actual IDL. The original
 * IDL can be found at `target/idl/leverage_delegate.json`.
 */
export type LeverageDelegate = {
  "address": "EPGF9iFrbGnhWgC3To9rC9vxinEYuDHaz4RXgLPvuRkp",
  "metadata": {
    "name": "leverageDelegate",
    "version": "2.0.0",
    "spec": "0.1.0",
    "description": "Permissionless leverage delegation strategies for Omnipair V2 (Dusk)"
  },
  "instructions": [
    {
      "name": "afterCloseOrder",
      "discriminator": [
        156,
        224,
        238,
        250,
        95,
        229,
        235,
        59
      ],
      "accounts": [
        {
          "name": "order",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  111,
                  114,
                  100,
                  101,
                  114
                ]
              },
              {
                "kind": "account",
                "path": "order.position",
                "account": "leverageOrder"
              },
              {
                "kind": "account",
                "path": "order.owner",
                "account": "leverageOrder"
              },
              {
                "kind": "arg",
                "path": "args.order_id"
              }
            ]
          }
        },
        {
          "name": "owner",
          "writable": true
        },
        {
          "name": "leveragePosition"
        },
        {
          "name": "leverageDelegation"
        },
        {
          "name": "custodyAuthority",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  100,
                  101,
                  108,
                  101,
                  103,
                  97,
                  116,
                  101,
                  95,
                  97,
                  117,
                  116,
                  104,
                  111,
                  114,
                  105,
                  116,
                  121
                ]
              },
              {
                "kind": "account",
                "path": "order"
              }
            ]
          }
        },
        {
          "name": "custodyTokenAccount",
          "writable": true
        },
        {
          "name": "executorTokenAccount",
          "writable": true
        },
        {
          "name": "ownerTokenAccount",
          "writable": true
        },
        {
          "name": "tokenMint"
        },
        {
          "name": "executor",
          "signer": true
        },
        {
          "name": "tokenProgram",
          "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        },
        {
          "name": "token2022Program",
          "address": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
        }
      ],
      "args": [
        {
          "name": "args",
          "type": {
            "defined": {
              "name": "executeOrderArgs"
            }
          }
        }
      ]
    },
    {
      "name": "beforeStopLoss",
      "discriminator": [
        246,
        62,
        82,
        232,
        168,
        199,
        121,
        78
      ],
      "accounts": [
        {
          "name": "order",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  111,
                  114,
                  100,
                  101,
                  114
                ]
              },
              {
                "kind": "account",
                "path": "leveragePosition"
              },
              {
                "kind": "account",
                "path": "order.owner",
                "account": "leverageOrder"
              },
              {
                "kind": "arg",
                "path": "args.order_id"
              }
            ]
          }
        },
        {
          "name": "market"
        },
        {
          "name": "leveragePosition"
        },
        {
          "name": "leverageDelegation"
        },
        {
          "name": "custodyAuthority",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  100,
                  101,
                  108,
                  101,
                  103,
                  97,
                  116,
                  101,
                  95,
                  97,
                  117,
                  116,
                  104,
                  111,
                  114,
                  105,
                  116,
                  121
                ]
              },
              {
                "kind": "account",
                "path": "order"
              }
            ]
          }
        },
        {
          "name": "custodyTokenAccount"
        },
        {
          "name": "tokenMint"
        },
        {
          "name": "executor",
          "signer": true
        }
      ],
      "args": [
        {
          "name": "args",
          "type": {
            "defined": {
              "name": "executeOrderArgs"
            }
          }
        }
      ]
    },
    {
      "name": "beforeTakeProfit",
      "discriminator": [
        5,
        35,
        111,
        0,
        223,
        131,
        193,
        31
      ],
      "accounts": [
        {
          "name": "order",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  111,
                  114,
                  100,
                  101,
                  114
                ]
              },
              {
                "kind": "account",
                "path": "leveragePosition"
              },
              {
                "kind": "account",
                "path": "order.owner",
                "account": "leverageOrder"
              },
              {
                "kind": "arg",
                "path": "args.order_id"
              }
            ]
          }
        },
        {
          "name": "market"
        },
        {
          "name": "leveragePosition"
        },
        {
          "name": "leverageDelegation"
        },
        {
          "name": "custodyAuthority",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  100,
                  101,
                  108,
                  101,
                  103,
                  97,
                  116,
                  101,
                  95,
                  97,
                  117,
                  116,
                  104,
                  111,
                  114,
                  105,
                  116,
                  121
                ]
              },
              {
                "kind": "account",
                "path": "order"
              }
            ]
          }
        },
        {
          "name": "custodyTokenAccount"
        },
        {
          "name": "tokenMint"
        },
        {
          "name": "executor",
          "signer": true
        }
      ],
      "args": [
        {
          "name": "args",
          "type": {
            "defined": {
              "name": "executeOrderArgs"
            }
          }
        }
      ]
    },
    {
      "name": "cancelLeverageOrder",
      "discriminator": [
        26,
        88,
        173,
        106,
        175,
        242,
        203,
        122
      ],
      "accounts": [
        {
          "name": "order",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  111,
                  114,
                  100,
                  101,
                  114
                ]
              },
              {
                "kind": "account",
                "path": "order.position",
                "account": "leverageOrder"
              },
              {
                "kind": "account",
                "path": "owner"
              },
              {
                "kind": "arg",
                "path": "args.order_id"
              }
            ]
          }
        },
        {
          "name": "owner",
          "writable": true,
          "signer": true
        }
      ],
      "args": [
        {
          "name": "args",
          "type": {
            "defined": {
              "name": "cancelLeverageOrderArgs"
            }
          }
        }
      ]
    },
    {
      "name": "createLeverageOrder",
      "discriminator": [
        197,
        206,
        10,
        223,
        89,
        46,
        93,
        17
      ],
      "accounts": [
        {
          "name": "market"
        },
        {
          "name": "leveragePosition"
        },
        {
          "name": "order",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  111,
                  114,
                  100,
                  101,
                  114
                ]
              },
              {
                "kind": "account",
                "path": "leveragePosition"
              },
              {
                "kind": "account",
                "path": "owner"
              },
              {
                "kind": "arg",
                "path": "args.order_id"
              }
            ]
          }
        },
        {
          "name": "owner",
          "writable": true,
          "signer": true
        },
        {
          "name": "systemProgram",
          "address": "11111111111111111111111111111111"
        }
      ],
      "args": [
        {
          "name": "args",
          "type": {
            "defined": {
              "name": "createLeverageOrderArgs"
            }
          }
        }
      ]
    },
    {
      "name": "updateLeverageOrder",
      "discriminator": [
        25,
        101,
        101,
        3,
        125,
        229,
        46,
        242
      ],
      "accounts": [
        {
          "name": "market"
        },
        {
          "name": "leveragePosition"
        },
        {
          "name": "order",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  108,
                  101,
                  118,
                  101,
                  114,
                  97,
                  103,
                  101,
                  95,
                  111,
                  114,
                  100,
                  101,
                  114
                ]
              },
              {
                "kind": "account",
                "path": "leveragePosition"
              },
              {
                "kind": "account",
                "path": "owner"
              },
              {
                "kind": "arg",
                "path": "args.order_id"
              }
            ]
          }
        },
        {
          "name": "owner",
          "writable": true,
          "signer": true
        }
      ],
      "args": [
        {
          "name": "args",
          "type": {
            "defined": {
              "name": "updateLeverageOrderArgs"
            }
          }
        }
      ]
    }
  ],
  "accounts": [
    {
      "name": "leverageDelegation",
      "discriminator": [
        49,
        60,
        29,
        23,
        243,
        219,
        16,
        214
      ]
    },
    {
      "name": "leverageOrder",
      "discriminator": [
        232,
        162,
        45,
        148,
        106,
        106,
        37,
        132
      ]
    },
    {
      "name": "leveragePosition",
      "discriminator": [
        88,
        78,
        124,
        68,
        228,
        129,
        34,
        251
      ]
    },
    {
      "name": "market",
      "discriminator": [
        219,
        190,
        213,
        55,
        0,
        227,
        198,
        154
      ]
    }
  ],
  "errors": [
    {
      "code": 6000,
      "name": "invalidOrder",
      "msg": "Invalid leverage order"
    },
    {
      "code": 6001,
      "name": "triggerNotMet",
      "msg": "Order trigger is not met"
    },
    {
      "code": 6002,
      "name": "invalidTokenAccount",
      "msg": "Invalid token account"
    },
    {
      "code": 6003,
      "name": "mathOverflow",
      "msg": "Math overflow"
    },
    {
      "code": 6004,
      "name": "approvalSerializationFailed",
      "msg": "Approval serialization failed"
    },
    {
      "code": 6005,
      "name": "invalidMarketVersion",
      "msg": "Unsupported Dusk market version"
    }
  ],
  "types": [
    {
      "name": "ammConfig",
      "docs": [
        "AMM controls. `peak_depth_nad == 0 && fade_scale_nad == 0` selects CPMM."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "peakDepthNad",
            "type": "u64"
          },
          {
            "name": "fadeScaleNad",
            "type": "u64"
          },
          {
            "name": "centerEmaHalfLifeMs",
            "type": "u64"
          },
          {
            "name": "volatilityHalfLifeMs",
            "type": "u64"
          },
          {
            "name": "adjustmentThresholdNad",
            "type": "u64"
          },
          {
            "name": "adjustmentStepNad",
            "type": "u64"
          },
          {
            "name": "minAdjustmentIntervalSlots",
            "type": "u64"
          },
          {
            "name": "volatilityShockCapNad",
            "type": "u64"
          },
          {
            "name": "volatilityCapNad",
            "type": "u64"
          },
          {
            "name": "divergenceFeeCoefficientNad",
            "type": "u64"
          },
          {
            "name": "volatilityFeeCoefficientNad",
            "type": "u64"
          },
          {
            "name": "concentrationRampDurationSlots",
            "type": "u64"
          },
          {
            "name": "reserved",
            "type": {
              "array": [
                "u8",
                33
              ]
            }
          }
        ]
      }
    },
    {
      "name": "ammState",
      "docs": [
        "Embedded mutable state for concentration, internal signals, protected",
        "liquidity, and an active parameter ramp."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "initialized",
            "type": "bool"
          },
          {
            "name": "appliedCurveParameters",
            "docs": [
              "Parameters already admitted by the protected-profit gate. Time alone",
              "never changes this field."
            ],
            "type": {
              "defined": {
                "name": "concentrationParameters"
              }
            }
          },
          {
            "name": "concentratedGeometryCache",
            "docs": [
              "Authoritative geometry for `applied_curve_parameters`. CPMM stores the",
              "all-zero cache. Only initialization or an admitted parameter change may",
              "replace it; center and reserve changes reuse it unchanged."
            ],
            "type": {
              "defined": {
                "name": "concentratedGeometryCache"
              }
            }
          },
          {
            "name": "centerPriceNad",
            "type": "u64"
          },
          {
            "name": "priceEmaNad",
            "type": "u64"
          },
          {
            "name": "lastTradePriceNad",
            "type": "u64"
          },
          {
            "name": "lastObservationSlot",
            "type": "u64"
          },
          {
            "name": "lastAdjustmentSlot",
            "type": "u64"
          },
          {
            "name": "lastConcentrationRampUpdateSlot",
            "docs": [
              "Prevents repeated instructions in one slot from advancing a ramp more",
              "than once."
            ],
            "type": "u64"
          },
          {
            "name": "volatilityAccumulatorNad",
            "type": "u64"
          },
          {
            "name": "invariantDNad",
            "type": "u128"
          },
          {
            "name": "curveMathRevision",
            "docs": [
              "Curve formula revision represented by `invariant_d_nad`."
            ],
            "type": "u8"
          },
          {
            "name": "qPerShareNad",
            "type": "u128"
          },
          {
            "name": "protectedFloorPerShareNad",
            "docs": [
              "yLP principal floor protected from funded recenter/ramp impairment."
            ],
            "type": "u128"
          },
          {
            "name": "retentionRequiredNad",
            "docs": [
              "Fresh protected-profit target that arms retained surcharge routing.",
              "This is a principal-budget target, never a cap on trader fees."
            ],
            "type": "u128"
          },
          {
            "name": "retentionStopNad",
            "docs": [
              "Hysteresis threshold below which retention remains armed."
            ],
            "type": "u128"
          },
          {
            "name": "retentionHardCapNad",
            "docs": [
              "Maximum protected principal one controller target may request/spend.",
              "It does not clip divergence or volatility surcharge amounts."
            ],
            "type": "u128"
          },
          {
            "name": "retainDynamicSurcharge",
            "docs": [
              "When true, dynamic surcharge is reserve principal; when false, the",
              "identical trader charge is routed to claimable yLP fee accounting."
            ],
            "type": "bool"
          },
          {
            "name": "retentionTargetSaturated",
            "docs": [
              "The requested protection target exceeded its principal-budget cap."
            ],
            "type": "bool"
          },
          {
            "name": "concentrationRamp",
            "type": {
              "defined": {
                "name": "concentrationRamp"
              }
            }
          },
          {
            "name": "retentionTargetStale",
            "docs": [
              "Retained surcharge changed executable inventory after the last exact",
              "forward-target solve. While stale, retention stays on until a decision",
              "point refreshes the target or executes a funded recenter."
            ],
            "type": "bool"
          },
          {
            "name": "deferredControllerTarget",
            "docs": [
              "Exact unfunded controller target retried by later real operations."
            ],
            "type": {
              "defined": {
                "name": "deferredControllerTarget"
              }
            }
          },
          {
            "name": "reserved",
            "type": {
              "array": [
                "u8",
                0
              ]
            }
          }
        ]
      }
    },
    {
      "name": "cancelLeverageOrderArgs",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "orderId",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "concentratedGeometryCache",
      "docs": [
        "Parameter-bound authoritative geometry persisted by market state.",
        "",
        "Q80 derivation is intentionally paid only when the applied shape changes.",
        "Ordinary quotes reconstruct every Q64/Q48 projection with shifts."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "mathRevision",
            "type": "u8"
          },
          {
            "name": "peakDepthNad",
            "type": "u64"
          },
          {
            "name": "fadeScaleNad",
            "type": "u64"
          },
          {
            "name": "peakQ80",
            "type": "u128"
          },
          {
            "name": "scaleQ80",
            "type": "u128"
          },
          {
            "name": "vStartQ80",
            "type": "u128"
          },
          {
            "name": "vTailQ80",
            "type": "u128"
          },
          {
            "name": "reserveRatioStartQ80",
            "type": "u128"
          },
          {
            "name": "reserveRatioTailQ80",
            "type": "u128"
          },
          {
            "name": "negativeQPrimeStartQ80",
            "type": "u128"
          }
        ]
      }
    },
    {
      "name": "concentrationParameters",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "peakDepthNad",
            "type": "u64"
          },
          {
            "name": "fadeScaleNad",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "concentrationRamp",
      "docs": [
        "A linear ramp whose governance delay is enforced by a queued parameter",
        "proposal. The ramp begins in the slot where that proposal executes."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "active",
            "type": "bool"
          },
          {
            "name": "start",
            "type": {
              "defined": {
                "name": "concentrationParameters"
              }
            }
          },
          {
            "name": "target",
            "type": {
              "defined": {
                "name": "concentrationParameters"
              }
            }
          },
          {
            "name": "startSlot",
            "type": "u64"
          },
          {
            "name": "endSlot",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "createLeverageOrderArgs",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "orderId",
            "type": "u64"
          },
          {
            "name": "kind",
            "type": "u8"
          },
          {
            "name": "triggerCloseoutPriceNad",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "dailyBorrowBucket",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "borrowedBucket",
            "docs": [
              "Gross principal lent out through the public borrow path. Internal hLP",
              "funding and isolated leverage do not consume this capacity. This is a",
              "24-hour leaky/token bucket, not an exact trailing-window sum: it permits",
              "a full burst after idle and then refills at the configured daily rate."
            ],
            "type": "u64"
          },
          {
            "name": "lastDecaySlot",
            "type": "u64"
          },
          {
            "name": "decayRemainderMs",
            "docs": [
              "Numerator remainder from `limit * elapsed_ms / MS_PER_DAY`. For a fixed",
              "absolute limit, carrying it makes refill independent of how often the",
              "bucket is checkpointed. The bps-derived absolute limit can still move",
              "when conservative market depth changes."
            ],
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "debt",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "fixedBaseShares",
            "type": "u128"
          },
          {
            "name": "fixedQuoteShares",
            "type": "u128"
          },
          {
            "name": "baseBorrowIndexNad",
            "type": "u128"
          },
          {
            "name": "quoteBorrowIndexNad",
            "type": "u128"
          },
          {
            "name": "baseRateAtTargetNad",
            "type": "u128"
          },
          {
            "name": "quoteRateAtTargetNad",
            "type": "u128"
          },
          {
            "name": "globalHealthBaseContributionForQuoteDebt",
            "type": "u64"
          },
          {
            "name": "globalHealthQuoteContributionForBaseDebt",
            "type": "u64"
          },
          {
            "name": "baseLastAccrualSlot",
            "type": "u64"
          },
          {
            "name": "quoteLastAccrualSlot",
            "type": "u64"
          },
          {
            "name": "fixedBasePrincipal",
            "docs": [
              "Aggregate outstanding *principal* (borrowed token amount, excluding",
              "accrued interest) backing fixed margin debt on each side. Accrued",
              "interest is `fixed_*_debt - fixed_*_principal`; tracked so interest can",
              "be routed to the interest vault (non-compounding) instead of",
              "compounding into reserves. Principal is a raw token-atom balance and is",
              "therefore bounded by the corresponding `u64` reserve custody domain."
            ],
            "type": "u64"
          },
          {
            "name": "fixedQuotePrincipal",
            "type": "u64"
          },
          {
            "name": "isolatedBaseShares",
            "docs": [
              "Aggregate isolated leverage debt. This debt contributes to utilization",
              "and interest, but is intentionally not utilized as normal margin debt.",
              "Shares remain `u128`; raw principal remains in the token account's",
              "`u64` amount domain."
            ],
            "type": "u128"
          },
          {
            "name": "isolatedQuoteShares",
            "type": "u128"
          },
          {
            "name": "isolatedBasePrincipal",
            "type": "u64"
          },
          {
            "name": "isolatedQuotePrincipal",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "deferredControllerTarget",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "kind",
            "docs": [
              "0 = none, 1 = parameter ramp, 2 = center move."
            ],
            "type": "u8"
          },
          {
            "name": "centerPriceNad",
            "type": "u64"
          },
          {
            "name": "parameters",
            "type": {
              "defined": {
                "name": "concentrationParameters"
              }
            }
          },
          {
            "name": "requiredNad",
            "type": "u128"
          },
          {
            "name": "evaluatedBaseReserveNad",
            "type": "u128"
          },
          {
            "name": "evaluatedQuoteReserveNad",
            "type": "u128"
          },
          {
            "name": "createdSlot",
            "type": "u64"
          },
          {
            "name": "saturated",
            "type": "bool"
          }
        ]
      }
    },
    {
      "name": "executeOrderArgs",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "orderId",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "fees",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "swapFeeGrowthIndexQ64",
            "type": "u128"
          },
          {
            "name": "interestGrowthIndexQ64",
            "type": "u128"
          },
          {
            "name": "swapFeeGrowthRemainderScaled",
            "docs": [
              "Scaled fee entitlement not yet representable by the integer growth",
              "index. The corresponding whole-token backing already sits in",
              "`swap_fee_liability`; it must never be redistributed as unallocated",
              "revenue."
            ],
            "type": "u64"
          },
          {
            "name": "interestGrowthRemainderScaled",
            "docs": [
              "Interest counterpart of `swap_fee_growth_remainder_scaled`."
            ],
            "type": "u64"
          },
          {
            "name": "hlpFundingInterestGrowthRemainderScaled",
            "docs": [
              "Source-scoped Q64 carry for interest paid by hLP funding debt. Funding",
              "uses a non-hLP denominator, while public interest uses total yLP",
              "supply; sharing one carry across those denominators would eventually",
              "leak rounding entitlement between the two populations."
            ],
            "type": "u64"
          },
          {
            "name": "swapFeeCustodyBalance",
            "docs": [
              "Claimable swap fees physically held in the reserve vault but excluded",
              "from executable cash and live reserves."
            ],
            "type": "u64"
          },
          {
            "name": "interestVaultBalance",
            "type": "u64"
          },
          {
            "name": "swapFeeLiability",
            "type": "u64"
          },
          {
            "name": "interestLiability",
            "type": "u64"
          },
          {
            "name": "unallocatedSwapFeeLiability",
            "type": "u64"
          },
          {
            "name": "unallocatedInterestLiability",
            "type": "u64"
          },
          {
            "name": "swapProtocolFeeLiability",
            "type": "u64"
          },
          {
            "name": "swapBuybackFeeLiability",
            "type": "u64"
          },
          {
            "name": "interestProtocolFeeLiability",
            "type": "u64"
          },
          {
            "name": "interestBuybackFeeLiability",
            "type": "u64"
          },
          {
            "name": "referralInterestLiability",
            "type": "u64"
          },
          {
            "name": "feeAuctionReferenceMarket",
            "docs": [
              "Governance-approved reference market for fee-lane auctions. A default",
              "key permits only the sold market itself when it directly pairs the sold",
              "and accepted mints."
            ],
            "type": "pubkey"
          },
          {
            "name": "buybackAuctionReferenceMarket",
            "docs": [
              "Governance-approved reference market for buyback-lane auctions. A",
              "default key has the same direct-market-only meaning as above."
            ],
            "type": "pubkey"
          },
          {
            "name": "feeSwapAuctionEpoch",
            "type": {
              "defined": {
                "name": "protocolAuctionEpoch"
              }
            }
          },
          {
            "name": "feeInterestAuctionEpoch",
            "type": {
              "defined": {
                "name": "protocolAuctionEpoch"
              }
            }
          },
          {
            "name": "buybackSwapAuctionEpoch",
            "type": {
              "defined": {
                "name": "protocolAuctionEpoch"
              }
            }
          },
          {
            "name": "buybackInterestAuctionEpoch",
            "type": {
              "defined": {
                "name": "protocolAuctionEpoch"
              }
            }
          }
        ]
      }
    },
    {
      "name": "hlpVault",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "ylpVault",
            "type": "pubkey"
          },
          {
            "name": "ylpShares",
            "type": "u64"
          },
          {
            "name": "baseHlpLiveReserve",
            "docs": [
              "hLP-owned live reserve depth that is not backed by reserve cash or",
              "normal cash-backed debt. This is the explicit synthetic live component",
              "in `r_virtual = r_cash + r_cash_backed_debt + r_hlp_live`."
            ],
            "type": "u64"
          },
          {
            "name": "quoteHlpLiveReserve",
            "type": "u64"
          },
          {
            "name": "debtShares",
            "docs": [
              "Funding debt used by the hLP vault. It accrues interest and counts",
              "toward utilization, but is not same-side cash-backed reserve debt."
            ],
            "type": "u128"
          },
          {
            "name": "debtPrincipal",
            "docs": [
              "Raw borrowed token atoms; products and indexed shares stay `u128`."
            ],
            "type": "u64"
          },
          {
            "name": "hlpSupply",
            "type": "u64"
          },
          {
            "name": "residualExposure",
            "type": "i128"
          },
          {
            "name": "baseSwapFeeGrowthIndexQ64",
            "type": "u128"
          },
          {
            "name": "baseInterestGrowthIndexQ64",
            "type": "u128"
          },
          {
            "name": "quoteSwapFeeGrowthIndexQ64",
            "type": "u128"
          },
          {
            "name": "quoteInterestGrowthIndexQ64",
            "type": "u128"
          },
          {
            "name": "baseSwapFeeCheckpointQ64",
            "type": "u128"
          },
          {
            "name": "baseInterestCheckpointQ64",
            "type": "u128"
          },
          {
            "name": "quoteSwapFeeCheckpointQ64",
            "type": "u128"
          },
          {
            "name": "quoteInterestCheckpointQ64",
            "type": "u128"
          },
          {
            "name": "baseSwapFeeRemainderQ64",
            "docs": [
              "Aggregate sub-atom yLP entitlement carried across hLP checkpoints.",
              "These are distinct from each holder YieldAccount remainder: this layer",
              "converts vault-owned yLP growth into hLP growth without double-flooring."
            ],
            "type": "u64"
          },
          {
            "name": "baseInterestRemainderQ64",
            "type": "u64"
          },
          {
            "name": "quoteSwapFeeRemainderQ64",
            "type": "u64"
          },
          {
            "name": "quoteInterestRemainderQ64",
            "type": "u64"
          },
          {
            "name": "baseSwapFeeGrowthRemainderScaled",
            "docs": [
              "Sub-index distribution carry for the second, yLP-to-hLP allocation",
              "layer. Whole-token backing represented here has already left the",
              "corresponding `unallocated_*` bucket."
            ],
            "type": "u64"
          },
          {
            "name": "baseInterestGrowthRemainderScaled",
            "type": "u64"
          },
          {
            "name": "quoteSwapFeeGrowthRemainderScaled",
            "type": "u64"
          },
          {
            "name": "quoteInterestGrowthRemainderScaled",
            "type": "u64"
          },
          {
            "name": "unallocatedBaseSwapFeeAmount",
            "type": "u64"
          },
          {
            "name": "unallocatedBaseInterestAmount",
            "type": "u64"
          },
          {
            "name": "unallocatedQuoteSwapFeeAmount",
            "type": "u64"
          },
          {
            "name": "unallocatedQuoteInterestAmount",
            "type": "u64"
          },
          {
            "name": "lastNavNad",
            "type": "u128"
          },
          {
            "name": "cachedSettlementPriceNad",
            "type": "u128"
          }
        ]
      }
    },
    {
      "name": "insurance",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "baseVault",
            "type": "pubkey"
          },
          {
            "name": "quoteVault",
            "type": "pubkey"
          },
          {
            "name": "baseAvailable",
            "type": "u64"
          },
          {
            "name": "quoteAvailable",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "irmConfig",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "targetUtilizationBps",
            "type": "u16"
          },
          {
            "name": "curveSteepnessNad",
            "type": "u64"
          },
          {
            "name": "adjustmentSpeedPerYear",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "leverageDelegation",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "owner",
            "type": "pubkey"
          },
          {
            "name": "market",
            "type": "pubkey"
          },
          {
            "name": "position",
            "type": "pubkey"
          },
          {
            "name": "debtAsset",
            "type": "u8"
          },
          {
            "name": "delegatedProgram",
            "type": "pubkey"
          },
          {
            "name": "approvedActions",
            "type": "u32"
          },
          {
            "name": "bump",
            "type": "u8"
          }
        ]
      }
    },
    {
      "name": "leverageOrder",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "owner",
            "type": "pubkey"
          },
          {
            "name": "market",
            "type": "pubkey"
          },
          {
            "name": "position",
            "type": "pubkey"
          },
          {
            "name": "orderId",
            "type": "u64"
          },
          {
            "name": "kind",
            "type": "u8"
          },
          {
            "name": "triggerCloseoutPriceNad",
            "type": "u64"
          },
          {
            "name": "stagedMargin",
            "type": "u64"
          },
          {
            "name": "stagedCustodyTokenAccount",
            "type": "pubkey"
          },
          {
            "name": "stagedOutputMint",
            "type": "pubkey"
          },
          {
            "name": "stagedOutputAmount",
            "type": "u64"
          },
          {
            "name": "bump",
            "type": "u8"
          }
        ]
      }
    },
    {
      "name": "leveragePosition",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "owner",
            "type": "pubkey"
          },
          {
            "name": "market",
            "type": "pubkey"
          },
          {
            "name": "positionId",
            "type": "pubkey"
          },
          {
            "name": "referralPartner",
            "type": "pubkey"
          },
          {
            "name": "referralInterestShareBps",
            "type": "u16"
          },
          {
            "name": "debtAsset",
            "type": "u8"
          },
          {
            "name": "collateralAmount",
            "type": "u64"
          },
          {
            "name": "marginAmount",
            "type": "u64"
          },
          {
            "name": "openNotional",
            "type": "u64"
          },
          {
            "name": "debtPrincipal",
            "type": "u128"
          },
          {
            "name": "debtShares",
            "type": "u128"
          },
          {
            "name": "multiplierBps",
            "type": "u64"
          },
          {
            "name": "openedAt",
            "type": "i64"
          },
          {
            "name": "openedSlot",
            "type": "u64"
          },
          {
            "name": "bump",
            "type": "u8"
          }
        ]
      }
    },
    {
      "name": "market",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "version",
            "type": "u8"
          },
          {
            "name": "ylpMint",
            "type": "pubkey"
          },
          {
            "name": "baseSide",
            "type": {
              "defined": {
                "name": "marketSide"
              }
            }
          },
          {
            "name": "quoteSide",
            "type": {
              "defined": {
                "name": "marketSide"
              }
            }
          },
          {
            "name": "config",
            "type": {
              "defined": {
                "name": "marketConfig"
              }
            }
          },
          {
            "name": "amm",
            "type": {
              "defined": {
                "name": "ammState"
              }
            }
          },
          {
            "name": "debt",
            "type": {
              "defined": {
                "name": "debt"
              }
            }
          },
          {
            "name": "baseHlpVault",
            "type": {
              "defined": {
                "name": "hlpVault"
              }
            }
          },
          {
            "name": "quoteHlpVault",
            "type": {
              "defined": {
                "name": "hlpVault"
              }
            }
          },
          {
            "name": "risk",
            "type": {
              "defined": {
                "name": "risk"
              }
            }
          },
          {
            "name": "insurance",
            "type": {
              "defined": {
                "name": "insurance"
              }
            }
          },
          {
            "name": "paramsHash",
            "type": {
              "array": [
                "u8",
                32
              ]
            }
          },
          {
            "name": "governanceLockedYlp",
            "docs": [
              "External yLP burned into active governance support. This is added back",
              "when computing direct-yLP eligibility; internal reserve-share supply is",
              "intentionally unchanged by governance locking."
            ],
            "type": "u64"
          },
          {
            "name": "parameterRevisions",
            "docs": [
              "Independent monotone revisions for fee, concentration, IRM, EMA, and",
              "daily-borrow-limit parameter families, in that order."
            ],
            "type": {
              "array": [
                "u64",
                5
              ]
            }
          },
          {
            "name": "lastMarginalObservationNad",
            "docs": [
              "Latest trader-visible marginal price committed by a curve mutation."
            ],
            "type": "u64"
          },
          {
            "name": "curveRevision",
            "docs": [
              "Monotone revision for executable-curve mutations."
            ],
            "type": "u64"
          },
          {
            "name": "riskRevision",
            "docs": [
              "Curve revision represented by the materialized lending-risk snapshot."
            ],
            "type": "u64"
          },
          {
            "name": "lastUpdateSlot",
            "type": "u64"
          },
          {
            "name": "reduceOnly",
            "type": "bool"
          },
          {
            "name": "bump",
            "type": "u8"
          }
        ]
      }
    },
    {
      "name": "marketConfig",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "swapFeeBps",
            "type": "u16"
          },
          {
            "name": "divergenceFeeShareCapBps",
            "type": "u16"
          },
          {
            "name": "volatilityFeeShareCapBps",
            "type": "u16"
          },
          {
            "name": "targetHlpLeverageBps",
            "type": "u16"
          },
          {
            "name": "settlementDivergenceBps",
            "type": "u16"
          },
          {
            "name": "emaHalfLifeMs",
            "type": "u64"
          },
          {
            "name": "directionalEmaHalfLifeMs",
            "type": "u64"
          },
          {
            "name": "qEmaHalfLifeMs",
            "type": "u64"
          },
          {
            "name": "maxDailyBorrowBps",
            "type": "u16"
          },
          {
            "name": "globalHealthContributionCapBps",
            "type": "u16"
          },
          {
            "name": "borrowMarketHealthFloorBps",
            "type": "u16"
          },
          {
            "name": "amm",
            "type": {
              "defined": {
                "name": "ammConfig"
              }
            }
          },
          {
            "name": "irm",
            "type": {
              "defined": {
                "name": "irmConfig"
              }
            }
          },
          {
            "name": "startTime",
            "type": "i64"
          }
        ]
      }
    },
    {
      "name": "marketSide",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "assetMint",
            "type": "pubkey"
          },
          {
            "name": "assetDecimals",
            "type": "u8"
          },
          {
            "name": "hlpMint",
            "type": "pubkey"
          },
          {
            "name": "reserveVault",
            "type": "pubkey"
          },
          {
            "name": "collateralVault",
            "type": "pubkey"
          },
          {
            "name": "interestVault",
            "type": "pubkey"
          },
          {
            "name": "reserves",
            "type": {
              "defined": {
                "name": "reserves"
              }
            }
          },
          {
            "name": "shares",
            "type": {
              "defined": {
                "name": "reserveShares"
              }
            }
          },
          {
            "name": "fees",
            "type": {
              "defined": {
                "name": "fees"
              }
            }
          },
          {
            "name": "dailyBorrowBucket",
            "type": {
              "defined": {
                "name": "dailyBorrowBucket"
              }
            }
          }
        ]
      }
    },
    {
      "name": "protocolAuctionEpoch",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "startSlot",
            "type": "u64"
          },
          {
            "name": "trackedInventory",
            "docs": [
              "Liability remaining immediately after the preceding fill. A larger",
              "current liability proves that new inventory arrived and starts a new",
              "epoch instead of inheriting an old floor price."
            ],
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "reserveShares",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "ylpSupply",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "reserves",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "liveReserve",
            "type": "u64"
          },
          {
            "name": "cashReserve",
            "type": "u64"
          },
          {
            "name": "baseHlpBackingInventory",
            "docs": [
              "Physical reserve-vault atoms removed from executable AMM inventory by",
              "base-hLP deleveraging. They are conservation-only bookkeeping, excluded",
              "from hLP NAV and exit output, and return to executable cash pro rata as",
              "base hLP exits."
            ],
            "type": "u64"
          },
          {
            "name": "quoteHlpBackingInventory",
            "docs": [
              "Quote-hLP counterpart of `base_hlp_backing_inventory`; never a second",
              "hLP NAV or withdrawal claim."
            ],
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "risk",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "basePriceEmaNad",
            "type": "u64"
          },
          {
            "name": "quotePriceEmaNad",
            "type": "u64"
          },
          {
            "name": "directionalBasePriceEmaNad",
            "type": "u64"
          },
          {
            "name": "directionalQuotePriceEmaNad",
            "type": "u64"
          },
          {
            "name": "cachedSpotBasePriceNad",
            "type": "u64"
          },
          {
            "name": "cachedSpotQuotePriceNad",
            "type": "u64"
          },
          {
            "name": "cachedQNad",
            "docs": [
              "Last observed balanced-equivalent CONCENTRATED depth."
            ],
            "type": "u128"
          },
          {
            "name": "qEmaNad",
            "docs": [
              "EMA of balanced-equivalent CONCENTRATED depth. This replaces the CPMM `K` EMA",
              "while retaining the same serialized width."
            ],
            "type": "u128"
          },
          {
            "name": "lastSnapshotSlot",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "updateLeverageOrderArgs",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "orderId",
            "type": "u64"
          },
          {
            "name": "kind",
            "type": "u8"
          },
          {
            "name": "triggerCloseoutPriceNad",
            "type": "u64"
          }
        ]
      }
    }
  ]
};
